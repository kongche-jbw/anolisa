#!/usr/bin/env python3
"""Bind native Qoder prompt/tool lifecycles to explicit AW invocations."""

import copy
import fcntl
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid

LIMIT = 1024 * 1024


def read(path: Path) -> dict:
    return json.loads(path.read_text())


def write(path: Path, value: object) -> None:
    temporary = path.with_name(f".{path.name}-{uuid.uuid4().hex}")
    try:
        with temporary.open("x") as stream:
            os.chmod(temporary, 0o600)
            json.dump(value, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def ticks(pid: int) -> int:
    return int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19])


def authenticate(config: dict, event: dict) -> None:
    pid = config["agent_pid"]
    if ticks(pid) != config["agent_start_ticks"]:
        raise ValueError("Agent process generation changed")
    parent = os.getppid()
    for _ in range(64):
        if parent == pid:
            break
        if parent <= 1:
            raise ValueError("hook is not a descendant of the bound Agent process")
        parent = int(Path(f"/proc/{parent}/stat").read_text().rsplit(")", 1)[1].split()[1])
    else:
        raise ValueError("hook ancestry exceeds limit")
    if event.get("cwd") != config["workspace"] or event.get("agent_id"):
        raise ValueError("only the bound workspace and main Agent are supported")
    if not isinstance(event.get("session_id"), str) or not event["session_id"]:
        raise ValueError("native session identity missing")


def call_id(event: dict) -> str:
    tool = event.get("tool_use_id")
    if not isinstance(tool, str) or not tool:
        raise ValueError("native tool identity missing")
    return hashlib.sha256(json.dumps([event["session_id"], tool]).encode()).hexdigest()


def transition(root: Path, config: dict, event: dict) -> Path | None:
    """Snapshot each tool's turn before execution, including overlapping prompts."""
    kind = event["hook_event_name"]
    with (root / "state.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = read(root / "state.json")
        if kind == "SessionStart":
            if event["session_id"] != state["session_id"]:
                if event.get("source") not in ("new", "clear"):
                    raise ValueError("unexpected native session transition")
                state.update(
                    session_id=event["session_id"],
                    turn_id=None,
                    session_start_source=event["source"],
                )
                write(root / "state.json", state)
            return None
        if event["session_id"] != state["session_id"]:
            raise ValueError("event session differs from the active native session")
        if kind == "UserPromptSubmit":
            state.update(turn_id=str(uuid.uuid4()), sequence=state["sequence"] + 1)
            write(
                root / "turns" / f"{state['turn_id']}.json",
                {
                    **state,
                    "observed_at_ms": int(time.time() * 1000),
                    "source": "UserPromptSubmit",
                    "agent_pid": config["agent_pid"],
                    "agent_start_ticks": config["agent_start_ticks"],
                },
            )
            write(root / "state.json", state)
            return None
        if (
            kind not in ("PreToolUse", "PostToolUse", "PostToolUseFailure")
            or event.get("tool_name") != "Bash"
        ):
            raise ValueError("unsupported lifecycle event")
        if state["session_id"] != config["session_id"]:
            raise ValueError("native session changed; restart the AW launcher to enable processing")
        call = root / "calls" / call_id(event)
        if kind == "PreToolUse":
            if not state["turn_id"]:
                raise ValueError("Bash has no observed user-prompt turn")
            snapshot = {
                "session_id": state["session_id"],
                "turn_id": state["turn_id"],
                "tool_use_id": event["tool_use_id"],
                "tool_input": event["tool_input"],
                "started_at_ms": int(time.time() * 1000),
            }
            if call.exists():
                old = read(call / "before.json")
                if any(old[k] != snapshot[k] for k in ("session_id", "tool_use_id", "tool_input")):
                    raise ValueError("native tool identity reused with different input")
            else:
                call.mkdir(mode=0o700)
                write(call / "before.json", snapshot)
            return None
        before = read(call / "before.json")
        if before["tool_input"] != event["tool_input"]:
            raise ValueError("tool input changed after the pre-tool binding")
        if (call / "native.json").exists():
            raise ValueError("duplicate native post-tool event")
        write(call / "native.json", event)
        return call


def invoke(root: Path, config: dict, call: Path, event: dict) -> bytes:
    if event["hook_event_name"] == "PostToolUseFailure":
        write(
            call / "completed.json",
            {
                "returncode": 1,
                "finished_at_ms": int(time.time() * 1000),
                "reason": "native Bash failure; not eligible for AW projection",
            },
        )
        return b"{}"
    before = read(call / "before.json")
    settings = copy.deepcopy(config["aw"])
    settings["scope"].update(session_id=before["session_id"], turn_id=before["turn_id"])
    settings["qoder_single_turn_id"] = before["turn_id"]
    # Publish only a completed immutable event into the shared verifier directory.
    settings["evidence"] = str(call / "evidence")
    write(call / "settings.json", settings)
    with (call / "hook.stderr.log").open("wb") as log:
        result = subprocess.run(
            [config["hook_bin"], "qoder", str(call / "settings.json")],
            input=json.dumps(event).encode(),
            stdout=subprocess.PIPE,
            stderr=log,
            timeout=25,
            check=False,
        )
    completion = {"returncode": result.returncode, "finished_at_ms": int(time.time() * 1000)}
    if result.returncode == 0:
        files = list((call / "evidence").glob("*.json"))
        if len(files) != 1:
            raise ValueError("expected exactly one completed Core event")
        evidence = read(files[0])
        os.link(files[0], root / "evidence" / files[0].name)
        completion["event_key"] = evidence["event_key"]
        write(
            call / "adoption-binding.json",
            {
                **{
                    key: config["aw"][key]
                    for key in ("runtime", "agent_pid", "agent_start_ticks", "journal")
                },
                "evidence": str(root / "evidence"),
                "scope": evidence["execution"]["scope"],
                "workspace": config["workspace"],
                "started_at_ms": config["started_at_ms"],
                "native_event": str(call / "native.json"),
                "adoption_journal": str(root / "adoption-journal"),
            },
        )
    write(call / "completed.json", completion)
    if result.returncode:
        return (
            b'{"systemMessage":"AW could not process this Bash result; original output retained."}'
        )
    return result.stdout


def main() -> int:
    os.umask(0o077)
    root = Path(sys.argv[1]).resolve()
    event = {}
    try:
        raw = sys.stdin.buffer.read(LIMIT + 1)
        if len(raw) > LIMIT:
            raise ValueError("native hook input exceeds 1 MiB")
        event = json.loads(raw)
        config = read(root / "runtime.json")
        authenticate(config, event)
        call = transition(root, config, event)
        sys.stdout.buffer.write(invoke(root, config, call, event) if call else b"{}")
        return 0
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        write(
            root / "errors" / f"{uuid.uuid4().hex}.json",
            {
                "message": str(error),
                "observed_at_ms": int(time.time() * 1000),
                "session_id": event.get("session_id"),
            },
        )
        print(
            json.dumps(
                {
                    "systemMessage": "AW lifecycle verification unavailable; original tool behavior retained."
                }
            )
        )
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
