"""Reproduce Agent group cleanup after the leader exits, without native agents."""

import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
import session
from session_hooks import ticks, write


class CleanupTests(unittest.TestCase):
    def test_exited_leader_does_not_leave_term_ignoring_child(self):
        # Reap the orphan ourselves so the test does not depend on PID 1's policy.
        libc = ctypes.CDLL(None, use_errno=True)
        previous = ctypes.c_int()
        self.assertEqual(libc.prctl(37, ctypes.byref(previous), 0, 0, 0), 0)
        self.assertEqual(libc.prctl(36, 1, 0, 0, 0), 0)
        with tempfile.TemporaryDirectory(prefix="cleanup-unit-", dir=AW / "target") as tmp:
            root = Path(tmp)
            command = [
                sys.executable,
                "-c",
                """
import os, signal, sys, time
from pathlib import Path
root = Path(sys.argv[1])
child = os.fork()
if child == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    (root / "child").write_text(str(os.getpid()))
    time.sleep(20)
    os._exit(0)
end = time.monotonic() + 5
while not (root / "exit").exists() and time.monotonic() < end:
    time.sleep(0.01)
os._exit(0)
""",
                str(root),
            ]
            process = subprocess.Popen(command, cwd=root, start_new_session=True)
            child = reaper = None
            write(
                root / "ownership.json",
                {
                    "command": command,
                    "cwd": tmp,
                    "pid": process.pid,
                    "pgid": process.pid,
                    "stop": f"kill -KILL -- -{process.pid}",
                    "ports": [],
                    "log": None,
                    "lifetime_seconds": 20,
                },
            )
            try:
                write(
                    root / "runtime.json",
                    {
                        "agent_pid": process.pid,
                        "agent_start_ticks": ticks(process.pid),
                        "agent_pgid": process.pid,
                    },
                )
                deadline = time.monotonic() + 5
                while not (root / "child").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                child = int((root / "child").read_text())
                (root / "exit").touch()
                process.wait(timeout=5)
                reaper = threading.Thread(target=os.waitpid, args=(child, 0))
                reaper.start()
                session.stop_agent(root)
                reaper.join(timeout=1)
                self.assertFalse(reaper.is_alive(), "registered group retained a live child")
                with self.assertRaises(ProcessLookupError):
                    os.killpg(process.pid, 0)
            finally:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=5)
                if reaper:
                    reaper.join(timeout=5)
                    self.assertFalse(reaper.is_alive())
                elif child:
                    os.waitpid(child, 0)
                self.assertEqual(libc.prctl(36, previous.value, 0, 0, 0), 0)
                self.assertFalse(Path(f"/proc/{process.pid}").exists())
                if child:
                    self.assertFalse(Path(f"/proc/{child}").exists())
        self.assertFalse(root.exists())

    def test_pid_reuse_is_not_signalled(self):
        with (
            patch(
                "session.read",
                return_value={"agent_pid": 12, "agent_start_ticks": 10, "agent_pgid": 12},
            ),
            patch("pathlib.Path.exists", return_value=True),
            patch("session.ticks", return_value=11),
            patch("session.os.killpg") as kill,
        ):
            with self.assertRaisesRegex(RuntimeError, "PID was reused"):
                session.stop_agent(Path("unused"))
            kill.assert_not_called()


if __name__ == "__main__":
    unittest.main()
