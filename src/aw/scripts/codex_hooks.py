#!/usr/bin/env python3
"""Inspect native Codex Bash results within the launcher-owned process and pane."""

import copy
import fcntl
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
import uuid

from session_hooks import LIMIT, authenticate, call_id, read, write


def arguments() -> list[str]:
    # Keep the reviewed command stable across panes; authenticate its environment
    # against the native process before accepting any event.
    command = shlex.join([sys.executable, str(Path(__file__).resolve())])
    handler = '{type="command",command=' + json.dumps(command) + ",timeout=30}"
    return [
        "--enable",
        "hooks",
        "-c",
        "hooks.SessionStart=[{hooks=[" + handler + "]}]",
        "-c",
        'hooks.PostToolUse=[{matcher="^Bash$",hooks=[' + handler + "]}]",
    ]


def transition(root: Path, event: dict) -> Path | None:
    kind = event["hook_event_name"]
    if kind not in ("SessionStart", "PostToolUse"):
        raise ValueError("unsupported Codex hook event")
    if kind == "PostToolUse" and (
        event.get("tool_name") != "Bash"
        or not isinstance(event.get("turn_id"), str)
        or not event["turn_id"]
    ):
        raise ValueError("Codex Bash result lacks its native turn identity")
    with (root / "state.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state = read(root / "state.json")
        if state["session_id"] is None:
            state.update(session_id=event["session_id"], session_start_source="startup")
        elif state["session_id"] != event["session_id"]:
            state["session_changed"] = True
            write(root / "state.json", state)
            raise ValueError("Codex session changed; exit and launch codex again")
        state["hook_connected"] = True
        write(root / "state.json", state)
        if kind == "SessionStart":
            return None
        call = root / "calls" / call_id(event)
        call.mkdir(mode=0o700)
        write(
            call / "before.json",
            {key: event[key] for key in ("session_id", "turn_id", "tool_use_id")},
        )
        write(call / "native.json", event)
        return call


def invoke(root: Path, config: dict, call: Path, event: dict) -> bytes:
    settings = copy.deepcopy(config["aw"])
    settings["scope"].update({key: event[key] for key in ("session_id", "turn_id")})
    settings["evidence"] = str(call / "evidence")
    write(call / "settings.json", settings)
    with (call / "hook.stderr.log").open("wb") as log:
        result = subprocess.run(
            [config["hook_bin"], "codex", str(call / "settings.json")],
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
            raise ValueError("expected exactly one completed Core inspection")
        evidence = read(files[0])
        os.link(files[0], root / "evidence" / files[0].name)
        completion["event_key"] = evidence["event_key"]
    write(call / "completed.json", completion)
    if result.returncode:
        return b'{"systemMessage":"AW SecCore inspection failed; original tool output retained."}'
    return result.stdout


def main() -> int:
    os.umask(0o077)
    root = Path(os.environ["AW_CODEX_SESSION_DIR"]).resolve()
    event = {}
    try:
        raw = sys.stdin.buffer.read(LIMIT + 1)
        if len(raw) > LIMIT:
            raise ValueError("native hook input exceeds 1 MiB")
        event = json.loads(raw)
        config = read(root / "runtime.json")
        if config["agent_kind"] != "codex" or config["aw"].get("tokenless"):
            raise ValueError("Codex requires an inspection-only runtime")
        authenticate(config, event)
        call = transition(root, event)
        sys.stdout.buffer.write(invoke(root, config, call, event) if call else b"{}")
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        write(
            root / "errors" / f"{uuid.uuid4().hex}.json",
            {
                "message": str(error),
                "observed_at_ms": int(time.time() * 1000),
                "session_id": event.get("session_id"),
            },
        )
        print('{"systemMessage":"AW Codex inspection unavailable; original tool output retained."}')
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
