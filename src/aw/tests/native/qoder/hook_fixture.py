#!/usr/bin/env python3
"""Native hook test fixture: trace start/end and emit an exact native response."""

import argparse
import json
import os
from pathlib import Path
import sys
import time


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--trace", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--delay", type=float, default=0.0)
    parser.add_argument("--response", type=Path)
    parser.add_argument("--when-command-contains")
    parser.add_argument("--exit-code", type=int, default=0)
    parser.add_argument("--stderr", default="")
    args = parser.parse_args()
    payload = json.load(sys.stdin)

    def trace(phase: str) -> None:
        record = {"name": args.name, "phase": phase, "time_ns": time.monotonic_ns(),
                  "pid": os.getpid(), "payload": payload}
        descriptor = os.open(args.trace, os.O_CREAT | os.O_WRONLY | os.O_APPEND, 0o600)
        try:
            os.write(descriptor, (json.dumps(record) + "\n").encode())
        finally:
            os.close(descriptor)

    trace("start")
    time.sleep(args.delay)
    trace("end")
    matches = (args.when_command_contains is None or args.when_command_contains in
               payload.get("tool_input", {}).get("command", ""))
    if args.response and matches:
        sys.stdout.write(args.response.read_text())
    else:
        print("{}")
    if args.stderr:
        print(args.stderr, file=sys.stderr)
    return args.exit_code


if __name__ == "__main__":
    raise SystemExit(main())
