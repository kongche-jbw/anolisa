"""Synthetic Qoder/cosh peers for launcher tests, never native Agent evidence."""

import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time

args = sys.argv[1:]
if Path(sys.argv[0]).name == "cosh-shell":
    if args == ["--version"]:
        print("cosh-shell 0.15.0")
    else:
        assert args[0] == "--"
        raise SystemExit(subprocess.call(args[1:]))
    raise SystemExit(0)
if args == ["--version"]:
    print("1.1.47")
    raise SystemExit(0)
if args[-3:] == ["plugins", "list", "--json"]:
    print("[]")
    raise SystemExit(0)


def argument(name):
    return args[args.index(name) + 1]


work = Path(argument("--cwd"))
mode = argument("-p")
Path(work / "agent-started").write_text(str(os.getpid()))
if mode in ("timeout", "early_exit"):
    pid = os.fork()
    if pid == 0:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        time.sleep(30)
        os._exit(0)
    (work / "descendant.pid").write_text(str(pid))
    if mode == "early_exit":
        raise SystemExit(7)
    time.sleep(30)
    raise SystemExit(0)
session = argument("--session-id")
native = Path(argument("--config-dir"))
history = native / "projects" / str(work).replace("/", "-").replace("_", "-") / f"{session}.jsonl"
history.parent.mkdir(parents=True, exist_ok=True)
golden = json.loads((work / "golden.json").read_text())
text = golden["request"]["input"]["content"]
tool = {"command": "printf synthetic", "description": "synthetic peer"}
row = {
    "type": "assistant",
    "sessionId": session,
    "cwd": str(work),
    "isSidechain": False,
    "message": {"content": [{"type": "tool_use", "id": "tool-1", "name": "Bash", "input": tool}]},
}
history.write_text(json.dumps(row) + "\n")
payload = {
    "hook_event_name": "PostToolUse",
    "session_id": session,
    "tool_use_id": "tool-1",
    "tool_name": "Bash",
    "tool_input": tool,
    "cwd": str(work),
    "transcript_path": str(history),
    "tool_response": {
        "kind": "completed",
        "exitCode": 0,
        "signal": None,
        "interrupted": False,
        "isImage": False,
        "noOutputExpected": False,
        "stderr": "",
        "stdout": text,
    },
}
if mode == "wrong_session":
    payload["session_id"] = "other-session"
settings = json.loads(Path(argument("--settings")).read_text())
hook = settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"]
output = subprocess.run(
    shlex.split(hook), input=json.dumps(payload), text=True, capture_output=True, timeout=20
)
(work / "hook-exit").write_text(str(output.returncode))
if output.returncode:
    raise SystemExit(4)
response = json.loads(output.stdout)
projected = response.get("hookSpecificOutput", {}).get("updatedToolOutput", text)
if isinstance(projected, dict):
    projected = projected["stdout"]
row = {
    "type": "user",
    "sessionId": session,
    "cwd": str(work),
    "isSidechain": False,
    "message": {
        "content": [{"type": "tool_result", "tool_use_id": "tool-1", "content": projected}]
    },
}
with history.open("a") as stream:
    stream.write(json.dumps(row) + "\n")
