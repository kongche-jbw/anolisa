"""Synthetic native process used to verify Host transport, never a scanner."""

import json
import os
import pathlib
import sys
import time

if sys.argv[1:] == ["--version"]:
    pathlib.Path("version-called").touch()
    print(os.environ.get("VERSION", "agent-sec-cli 0.12.0"))
    raise SystemExit(0)

mode = os.environ.get("MODE", "clean")
if mode == "no_read":
    pathlib.Path("no_read.started").touch()
    time.sleep(10)
text = sys.stdin.buffer.read().decode("utf-8")
pathlib.Path("called.json").write_text(json.dumps({
    "stdin": text, "args": sys.argv[1:], "cwd": os.getcwd(), "environment": dict(os.environ)
}), encoding="utf-8")
if mode == "sleep":
    time.sleep(10)
if mode in {"child_pipe", "child_closed"}:
    child = os.fork()
    if child == 0:
        if mode == "child_closed":
            for fd in (0, 1, 2):
                os.close(fd)
        time.sleep(10)
        os._exit(0)
    pathlib.Path("child.pid").write_text(str(child), encoding="utf-8")
    if mode == "child_pipe":
        raise SystemExit(0)
if mode == "fail":
    print("private stderr must never leak", file=sys.stderr)
    raise SystemExit(7)
if mode == "stdout_limit":
    for _ in range(1000):
        print("x" * 65_536, flush=True)
    raise SystemExit(0)
if mode == "stderr_limit":
    for _ in range(1000):
        print("private stderr" * 4096, file=sys.stderr, flush=True)
    raise SystemExit(0)
if mode == "mutate":
    pathlib.Path("rules.yaml").write_text("changed", encoding="utf-8")
if mode == "malformed":
    print("not JSON: private result")
    raise SystemExit(0)

print(json.dumps({"ok": True, "verdict": "pass", "findings": [], "elapsed_ms": 0,
    "summary": {"total": 0, "by_type": {}, "by_category": {}, "by_severity": {},
        "source": "tool_output", "bytes_scanned": len(text.encode("utf-8")),
        "truncated": False, "custom_rules": {"status": "absent", "rule_count": 0,
            "runtime_error_count": 0, "budget_exhausted": False, "truncated": False}}}))
