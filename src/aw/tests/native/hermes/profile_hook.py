#!/usr/bin/env python3
"""Observe the native Hook's working directory without recording credentials."""

import json
import os
from pathlib import Path
import sys


def main() -> None:
    payload = json.load(sys.stdin)
    row = {"label": sys.argv[1], "pid": os.getpid(), "cwd": os.getcwd(),
           "hermes_home": os.environ.get("HERMES_HOME"), "payload": payload,
           "source_profile_sentinel_visible": "AW_HERMES_PROFILE_SENTINEL" in os.environ}
    path = Path(os.environ["AW_HERMES_TEST_TRACE"])
    descriptor = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o600)
    try:
        os.write(descriptor, (json.dumps(row) + "\n").encode())
    finally:
        os.close(descriptor)


if __name__ == "__main__":
    main()
