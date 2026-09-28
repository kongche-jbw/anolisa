"""Test provider: append a native payload, optionally block before execution."""

import json
import os
import sys

label, mode, trace = sys.argv[1:]
payload = json.load(sys.stdin)
fd = os.open(trace, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
try:
    os.write(fd, (json.dumps({"label": label, "payload": payload}) + "\n").encode())
finally:
    os.close(fd)
if mode == "deny":
    print("Denied by AW QwenPaw test", file=sys.stderr)
    sys.exit(2)
if mode == "ask":
    print('{"action":"ask"}')
