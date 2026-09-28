#!/usr/bin/env python3
"""Verify native hook scheduling without submitting any model prompt."""

import argparse
import json
import os
from pathlib import Path
import signal
import time

import pexpect


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--aw", type=Path)
    parser.add_argument("--socket", type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    fixture = Path(__file__).with_name("hook_fixture.py").resolve()
    ledger_path = root / "processes.json"
    results = []
    for sequential in (False, True):
        name = ("aw-" if args.aw else "") + ("sequential" if sequential else "parallel")
        trace = root / (name + "-native-lifecycle.jsonl")
        if trace.exists():
            raise FileExistsError(trace)
        hooks = []
        for identifier in ("one", "two"):
            command = "/usr/bin/python3"
            command_args = [str(fixture), "--trace", str(trace), "--name", identifier,
                            "--delay", "0.5"]
            if args.aw:
                command = str(args.aw.resolve())
                command_args = ["hook", "--socket", str(args.socket), "--agent", "qoder",
                                "--event", "tool.before", "--provider", name + "-" + identifier]
            hooks.append({"type": "command", "command": command, "args": command_args,
                          "timeout": 5})
        settings = root / (name + "-native-lifecycle-settings.json")
        settings.write_text(json.dumps({"hooks": {"SessionStart": [
            {"sequential": sequential, "hooks": hooks}]}}))
        command = [str(root / "bin/qodercli"), "--config-dir", str(args.config),
                   "--cwd", str(root / "project"), "--settings", str(settings),
                   "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}']
        environment = dict(os.environ, TERM="xterm-256color")
        child = pexpect.spawn(command[0], command[1:], env=environment, encoding="utf-8",
                              timeout=0.2, dimensions=(42, 130))
        ledger = json.loads(ledger_path.read_text())
        record = {"command": command, "cwd": str(root / "project"), "pid": child.pid,
                  "process_group": child.pid, "ports": [], "log": str(trace),
                  "deadline_seconds": 45, "stop_command": f"kill -TERM -- -{child.pid}"}
        ledger["processes"].append(record)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        started = time.monotonic()
        try:
            while time.monotonic() - started < 45:
                try:
                    child.read_nonblocking(65536, timeout=0.2)
                except pexpect.TIMEOUT:
                    pass
                except pexpect.EOF:
                    break
                if trace.exists() and len(trace.read_text().splitlines()) == 4:
                    break
            events = [json.loads(line) for line in trace.read_text().splitlines()]
            assert len(events) == 4, events
            phases = [event["phase"] for event in events]
            expected = ["start", "end", "start", "end"] if sequential else ["start", "start", "end", "end"]
            assert phases == expected, phases
            results.append({"case": name, "phases": phases, "model_prompt_submitted": False,
                            "elapsed_seconds": round(time.monotonic() - started, 3), "passed": True})
        finally:
            child.sendcontrol("c")
            child.sendcontrol("c")
            try:
                child.expect(pexpect.EOF, timeout=3)
            except (pexpect.TIMEOUT, pexpect.EOF):
                pass
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            child.close(force=True)
            record.update(finished=True, exit_code=child.exitstatus, signal=child.signalstatus)
            ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
    (root / (("aw-" if args.aw else "") + "native-lifecycle-result.json")).write_text(
        json.dumps(results, indent=2) + "\n")
    print(json.dumps(results))


if __name__ == "__main__":
    main()
