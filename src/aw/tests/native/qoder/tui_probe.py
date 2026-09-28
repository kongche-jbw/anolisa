#!/usr/bin/env python3
"""Repeat a successful print fixture through the real Qoder terminal UI."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

import pexpect


def verify_transcript(root: Path, config: Path) -> dict:
    """Verify adoption from a completed turn; terminal redraws can split tokens."""
    trace = root / "hooks.jsonl"
    events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
    starts = [entry for entry in events if entry["phase"] == "start"]
    texts = []
    for session_id in {entry["payload"].get("session_id") for entry in starts}:
        for path in (config / "projects").glob(f"*/{session_id}.jsonl"):
            entries = [json.loads(line) for line in path.read_text().splitlines()]
            completed = {entry.get("message", {}).get("id") for entry in entries
                         if entry.get("type") == "assistant" and
                         entry.get("message", {}).get("stop_reason") == "end_turn"}
            for entry in entries:
                message = entry.get("message", {})
                if entry.get("type") == "assistant" and message.get("id") in completed:
                    texts.extend(block.get("text", "") for block in message.get("content", [])
                                 if block.get("type") == "text")
    return {"interface": "tui", "final_answer": "\n".join(texts),
        "model_final_contains_replacement": any("AW_NATIVE_POST_71c5f6" in text for text in texts),
        "native_stop_completed": any(entry["name"] == "native-stop" and entry["phase"] == "end"
                                     for entry in events),
        "hook_starts": [entry["name"] for entry in starts],
        "raw_tool_result_seen": any(entry["name"] == "native-after" and
            entry["payload"].get("tool_response", {}).get("stdout") == "native-original" for entry in starts)}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-case", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--aw", type=Path, required=True)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    def interrupted(_signal: int, _frame: object) -> None:
        raise KeyboardInterrupt("bounded probe interrupted")
    signal.signal(signal.SIGTERM, interrupted)
    root = args.root.resolve()
    if args.verify_only:
        result = verify_transcript(root, args.config)
        (root / "verified-result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        assert result["model_final_contains_replacement"] and result["native_stop_completed"] and result["raw_tool_result_seen"], result
        return
    root.mkdir()
    source = args.source_case.resolve()
    for name in ("aw.json", "native.json", "before-one.json", "before-two.json",
                 "after-one.json", "after-two.json"):
        (root / name).write_text((source / name).read_text().replace(str(source), str(root)))
    fixture = Path(__file__).with_name("hook_fixture.py").resolve()
    native = json.loads((root / "native.json").read_text())
    native["hooks"]["SessionStart"] = [{"hooks": [{"type": "command",
        "command": "/usr/bin/python3", "args": [str(fixture), "--trace",
        str(root / "hooks.jsonl"), "--name", "native-ready"], "timeout": 5}]}]
    native["hooks"]["Stop"] = [{"hooks": [{"type": "command",
        "command": "/usr/bin/python3", "args": [str(fixture), "--trace",
        str(root / "hooks.jsonl"), "--name", "native-stop"], "timeout": 5}]}]
    (root / "native.json").write_text(json.dumps(native))
    state = root / "state"
    state.mkdir(mode=0o700)
    project = root / "project"
    project.mkdir()
    socket = args.config.parent / "qoder-tui.sock"
    aw = str(args.aw.resolve())
    ledger = {"processes": []}
    ledger_path = root / "processes.json"

    def record(pid: int, command: list[str], log: Path, seconds: int) -> dict:
        entry = {"pid": pid, "process_group": pid, "command": command,
                 "cwd": str(project), "ports": [], "log": str(log),
                 "deadline_seconds": seconds, "stop_command": f"kill -TERM -- -{pid}"}
        ledger["processes"].append(entry)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        return entry

    daemon_command = [aw, "serve", "--config", str(root / "aw.json"),
                      "--socket", str(socket), "--idle-timeout", "120"]
    with (root / "daemon.log").open("w") as daemon_log:
        daemon = subprocess.Popen(daemon_command, cwd=project, stdout=daemon_log,
                                  stderr=subprocess.STDOUT, start_new_session=True)
        daemon_record = record(daemon.pid, daemon_command, root / "daemon.log", 120)
        child = None
        output = ""
        try:
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and not socket.exists():
                if daemon.poll() is not None:
                    raise RuntimeError("AW exited before readiness")
                time.sleep(0.05)
            if not socket.exists():
                raise TimeoutError("AW socket readiness timed out")
            command = [aw, "run", "qoder", "--config", str(root / "aw.json"),
                "--state-dir", str(state), "--socket", str(socket), "--native-config",
                str(root / "native.json"), "--", "--config-dir", str(args.config),
                "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
                "--max-model-request-retries", "0",
                "--max-output-tokens", "512", "--allowed-tools", "Bash", "--tools", "Bash",
                "--system-prompt", "Perform exactly one requested Bash call and then finish. Never retry."]
            child = pexpect.spawn(command[0], command[1:], cwd=project,
                env=dict(os.environ, TERM="xterm-256color"), encoding="utf-8",
                timeout=0.2, dimensions=(42, 130))
            child_record = record(child.pid, command, root / "screen.txt", 90)
            deadline = time.monotonic() + 90
            ready_at = None
            submitted = False
            adopted = False
            events = []
            trusted = False
            while time.monotonic() < deadline and child.isalive():
                try:
                    output += child.read_nonblocking(65536, timeout=0.2)
                except pexpect.TIMEOUT:
                    pass
                except pexpect.EOF:
                    break
                clean = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output)
                clean = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", clean)
                (root / "screen.txt").write_text(clean)
                compact = re.sub(r"\s+", "", clean)
                if (not trusted and "Doyoutrustthefilesinthisfolder?" in compact
                        and str(project) in compact):
                    # Only trust the freshly created fixture directory in this probe.
                    child.send("\r")
                    trusted = True
                trace = root / "hooks.jsonl"
                events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
                if ready_at is None and any(e["name"] == "native-ready" and e["phase"] == "end" for e in events):
                    ready_at = time.monotonic()
                if ready_at is None and "Typeyourmessageor@path/to/file" in compact:
                    ready_at = time.monotonic()
                if not submitted and ready_at and time.monotonic() - ready_at > 3:
                    child.send("Call Bash exactly once: printf 'native-original'. Then report the tool output exactly. Do not call other tools.")
                    child.send("\r")
                    submitted = True
                completed = [e for e in events if e["phase"] == "end"]
                if (any(e["name"] == "native-stop" for e in completed)
                        and verify_transcript(root, args.config)["model_final_contains_replacement"]):
                    adopted = True
                    child.send("/exit\r")
                    try:
                        child.expect(pexpect.EOF, timeout=5)
                    except (pexpect.EOF, pexpect.TIMEOUT):
                        pass
                    break
            result = verify_transcript(root, args.config)
            result["prompt_submitted"] = submitted
            (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(json.dumps(result))
            assert adopted and result["raw_tool_result_seen"], result
        finally:
            if child:
                try:
                    os.killpg(child.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                child.close(force=True)
                child_record.update(finished=True, exit_code=child.exitstatus, signal=child.signalstatus)
            if daemon.poll() is None:
                subprocess.run([aw, "stop", "--socket", str(socket)], capture_output=True, timeout=5)
                try:
                    daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(daemon.pid, signal.SIGTERM)
                    daemon.wait(timeout=5)
            daemon_record.update(finished=True, exit_code=daemon.returncode)
            ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")


if __name__ == "__main__":
    main()
