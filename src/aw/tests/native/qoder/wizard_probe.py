#!/usr/bin/env python3
"""Drive the native BYOK wizard through a bounded, private PTY session."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import time

import pexpect


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--key-file", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    root = args.root.resolve()
    control = args.config.parent / "qoder-wizard-control.json"
    screen = args.config.parent / "qoder-wizard-screen.txt"
    command = [str(root / "bin/qodercli"), "--config-dir", str(args.config),
               "--cwd", str(root / "project"), "--setting-sources", "user"]
    child = pexpect.spawn(command[0], command[1:], encoding="utf-8", timeout=1,
                          dimensions=(42, 130))
    ledger_path = root / "processes.json"
    ledger = json.loads(ledger_path.read_text())
    record = {"command": command, "cwd": str(root / "project"), "pid": child.pid,
              "process_group": child.pid, "ports": [], "log": str(screen),
              "deadline_seconds": args.timeout,
              "stop_command": f"kill -TERM -- -{child.pid}"}
    ledger["processes"].append(record)
    ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
    seen = None
    output = ""
    key = ""
    deadline = time.monotonic() + args.timeout
    try:
        while time.monotonic() < deadline and child.isalive():
            try:
                output += child.read_nonblocking(65536, timeout=0.1)
            except pexpect.TIMEOUT:
                pass
            except pexpect.EOF:
                break
            if control.exists():
                instruction = control.read_text()
                if instruction != seen:
                    seen = instruction
                    data = json.loads(instruction)
                    if data.get("key"):
                        key = args.key_file.read_text().strip()
                        child.send(key)
                    elif data.get("stop"):
                        break
                    else:
                        child.send(data["send"])
            clean = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", output)
            clean = re.sub(r"\x1b\][^\x07]*(?:\x07|\x1b\\)", "", clean)
            if key:
                clean = clean.replace(key, "[REDACTED]")
            screen.write_text(clean[-25000:])
            screen.chmod(0o600)
    finally:
        try:
            os.killpg(child.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        child.close(force=True)
        record.update(finished=True, exit_code=child.exitstatus, signal=child.signalstatus)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")


if __name__ == "__main__":
    main()
