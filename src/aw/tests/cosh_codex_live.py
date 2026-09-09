#!/usr/bin/env python3
"""Check native Codex argument dispatch and return through a real cosh terminal."""

import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time
import uuid

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
from session_hooks import read, write


def main():
    root = AW / "target" / ("cosh-codex-" + uuid.uuid4().hex[:8])
    root.mkdir(mode=0o700)
    before = set((AW / "target/sessions").iterdir())
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
    command = [str(AW / "scripts/cosh"), "--isolated"]
    process = subprocess.Popen(
        command,
        cwd=root,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=dict(
            os.environ,
            PYTHONDONTWRITEBYTECODE="1",
            COSH_AUDIT_DIR=str(root / "cosh-audit"),
            COSH_SHELL_BOOTSTRAP_PATH="0",
        ),
        start_new_session=True,
    )
    write(
        root / "ownership.json",
        {
            "command": command,
            "cwd": str(root),
            "pid": process.pid,
            "lifetime_seconds": 60,
            "stop": f"kill -TERM {process.pid}",
            "log": str(root / "screen.ansi"),
        },
    )
    print(root, flush=True)
    session = None
    result = {"status": "failed"}
    try:
        time.sleep(1)
        os.write(master, b"codex --help\r")
        deadline = time.monotonic() + 45
        with (root / "screen.ansi").open("wb") as log:
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    log.write(os.read(master, 65536))
                    log.flush()
                created = set((AW / "target/sessions").iterdir()) - before
                if len(created) == 1:
                    session = created.pop()
                    ownership = (
                        read(session / "ownership.json")
                        if (session / "ownership.json").exists()
                        else {}
                    )
                    if "cleanup" in ownership:
                        runtime = read(session / "runtime.json")
                        assert runtime["agent_kind"] == "codex"
                        assert runtime["command"][0] == runtime["agent_program"]
                        assert "--help" in runtime["command"]
                        assert "hooks.PostToolUse=" in " ".join(runtime["command"])
                        assert not list((session / "evidence").glob("*.json"))
                        assert read(session / "provider-details.json")["status"] == "agent exited"
                        os.write(master, b"printf 'COSH_%s\\n' RETURNED\rexit\r")
                        break
            else:
                raise TimeoutError("Codex did not return to cosh within 45 seconds")
            output = bytearray()
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    chunk = os.read(master, 65536)
                    log.write(chunk)
                    output.extend(chunk)
                if process.poll() is not None:
                    break
            assert b"COSH_RETURNED" in output
            process.wait(timeout=5)
            assert process.returncode == 0
            result = {
                "status": "passed",
                "session": str(session),
                "codex": runtime["command"],
                "scope": "native help dispatch, Herdr lifecycle and return; no model request",
            }
    finally:
        if process.poll() is None:
            if session and (session / "ownership.json").exists():
                owner = read(session / "ownership.json")
                if "cleanup" not in owner:
                    os.kill(owner["launcher_pid"], signal.SIGTERM)
                    time.sleep(1)
            os.write(master, b"exit\r")
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)
        os.close(master)
        os.close(slave)
        if session and (session / "ownership.json").exists():
            owner = read(session / "ownership.json")
            assert not Path(owner["temporary_directory"]).exists()
            for key in ("server_pid", "client_pid", "observer_pid"):
                if key in owner:
                    assert not Path(f"/proc/{owner[key]}").exists()
            runtime = read(session / "runtime.json")
            assert not Path(f"/proc/{runtime['agent_pid']}").exists()
        assert not Path(f"/proc/{process.pid}").exists()
        result["cleanup"] = "owned processes and temporary namespace absent; logs retained"
        write(root / "result.json", result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
