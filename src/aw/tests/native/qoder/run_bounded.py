#!/usr/bin/env python3
"""Run one isolated native probe and record its exact lifecycle."""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--cwd", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--timeout", type=int, default=180)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    root = args.root.resolve()
    root.mkdir(parents=True, exist_ok=True)
    log_path = root / (args.name + ".log")
    ledger_path = root / "processes.json"
    ledger = json.loads(ledger_path.read_text()) if ledger_path.exists() else {"processes": []}
    with log_path.open("w") as output:
        child = subprocess.Popen(command, cwd=args.cwd, stdout=output,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        record = {"command": command, "cwd": str(args.cwd.resolve()), "pid": child.pid,
                  "process_group": child.pid, "ports": [], "log": str(log_path),
                  "deadline_seconds": args.timeout, "started_at": time.time(),
                  "stop_command": f"kill -TERM -- -{child.pid}"}
        ledger["processes"].append(record)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        result = 124
        try:
            result = child.wait(timeout=args.timeout)
        except subprocess.TimeoutExpired:
            record["timed_out"] = True
        finally:
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
            record.update(exit_code=result, finished=True, ended_at=time.time())
            ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
    print(json.dumps({"name": args.name, "exit_code": result, "log": str(log_path)}))
    return result


if __name__ == "__main__":
    raise SystemExit(main())
