"""Owned child cleanup across Provider groups and bounded utility capture."""

import ctypes
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

INTEGRATION = Path(__file__).resolve().parents[1]
AW = INTEGRATION.parents[1]
sys.path.insert(0, str(INTEGRATION))
from session_process import Processes


class ProcessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="qoder-process-", dir=AW / "target")
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def test_setup_failures_do_not_escape_child_or_selector_ownership(self):
        with Processes() as processes:
            with patch(
                "session_process.selectors.DefaultSelector", side_effect=OSError("no descriptors")
            ):
                with patch("session_process.subprocess.Popen") as spawn:
                    with self.assertRaises(OSError):
                        processes.run(self.command("pass"), self.root, 1)
                    spawn.assert_not_called()
            selector = Mock()
            with patch("session_process.selectors.DefaultSelector", return_value=selector):
                with patch("session_process.subprocess.Popen", side_effect=OSError("cannot spawn")):
                    with self.assertRaises(OSError):
                        processes.run(self.command("pass"), self.root, 1)
                    selector.close.assert_called_once()
        self.assert_reaped()

    def command(self, body):
        return [sys.executable, "-B", "-c", body]

    def assert_reaped(self):
        self.assertEqual(Processes.children(), [])
        for path in self.root.glob("*.pid"):
            self.assertFalse(Path(f"/proc/{path.read_text()}").exists(), path.name)

    def tree(self, separate, cancel=False):
        return f"""
import os,pathlib,signal,time
root=pathlib.Path({str(self.root)!r})
(root/'leader.pid').write_text(str(os.getpid()))
child=os.fork()
if child==0:
 if {separate!r}:os.setsid()
 (root/'child.pid').write_text(str(os.getpid()))
 def stop(*_):
  time.sleep(0.2)
  (root/'term-grace').write_text('completed')
  raise SystemExit(0)
 signal.signal(signal.SIGTERM,stop)
 grand=os.fork()
 if grand==0:
  os.setsid()
  signal.signal(signal.SIGTERM,signal.SIG_IGN)
  (root/'grand.pid').write_text(str(os.getpid()))
  time.sleep(30)
  os._exit(0)
 (root/'ready').touch()
 time.sleep(30)
 os._exit(0)
deadline=time.monotonic()+3
while not (root/'ready').exists():
 if time.monotonic()>deadline:raise RuntimeError('child readiness timeout')
 time.sleep(0.01)
if {cancel!r}:
 os.kill(os.getppid(),signal.SIGTERM)
 time.sleep(30)
"""

    def test_early_leader_exit_reaps_same_and_separate_groups_with_term_grace(self):
        for separate in (False, True):
            with self.subTest(separate=separate):
                for path in self.root.iterdir():
                    path.unlink()
                with Processes() as processes:
                    started = time.monotonic()
                    self.assertEqual(
                        processes.run(self.command(self.tree(separate)), self.root, 5), 0
                    )
                    self.assertGreaterEqual(time.monotonic() - started, 1)
                self.assertEqual((self.root / "term-grace").read_text(), "completed")
                self.assert_reaped()

    def test_cancellation_reaps_cross_group_descendants(self):
        with Processes() as processes:
            with self.assertRaisesRegex(RuntimeError, "session cancelled"):
                processes.run(self.command(self.tree(True, cancel=True)), self.root, 5)
        self.assert_reaped()

    def test_capture_limits_both_streams_without_disclosing_contents(self):
        for stream in ("stdout", "stderr"):
            with self.subTest(stream=stream), Processes() as processes:
                body = f"import os,pathlib,sys;pathlib.Path('flood.pid').write_text(str(os.getpid()));sys.{stream}.write('private' * 200000);sys.{stream}.flush()"
                started = time.monotonic()
                with self.assertRaisesRegex(RuntimeError, "response exceeded limit") as caught:
                    processes.capture(self.command(body), self.root, 5)
                self.assertNotIn("private", str(caught.exception))
                self.assertLess(time.monotonic() - started, 4)
                self.assert_reaped()

    def test_capture_stalled_descendant_pipe_times_out_and_reaps(self):
        body = """
import os,pathlib,signal,time
child=os.fork()
if child==0:
 os.setsid()
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 pathlib.Path('pipe.pid').write_text(str(os.getpid()))
 time.sleep(30)
 os._exit(0)
"""
        with Processes() as processes:
            with self.assertRaisesRegex(RuntimeError, "timed out"):
                processes.capture(self.command(body), self.root, 0.5)
        self.assert_reaped()

    def test_capture_accepts_exact_limit_and_redacts_failed_native_stderr(self):
        with Processes() as processes:
            result = processes.capture(
                self.command("import sys;sys.stdout.write('x'*65536);sys.stderr.write('y'*65536)"),
                self.root,
            )
            self.assertEqual(result, "x" * 65536)
            with self.assertRaisesRegex(RuntimeError, "startup utility failed") as caught:
                processes.capture(
                    self.command("import sys;print('private',file=sys.stderr);sys.exit(2)"),
                    self.root,
                )
            self.assertNotIn("private", str(caught.exception))
        self.assert_reaped()

    def test_preexisting_child_rejected_without_touching_it_on_enter_or_run(self):
        for already_entered in (False, True):
            processes = Processes()
            if already_entered:
                processes.__enter__()
            unrelated = subprocess.Popen(self.command("import time;time.sleep(30)"))
            try:
                with self.assertRaisesRegex(RuntimeError, "without existing children"):
                    if already_entered:
                        processes.run(self.command("raise SystemExit(0)"), self.root, 1)
                    else:
                        processes.__enter__()
                self.assertIsNone(unrelated.poll())
            finally:
                unrelated.terminate()
                unrelated.wait(timeout=3)
                if already_entered:
                    processes.__exit__()
        self.assert_reaped()

    def test_unrelated_sibling_survives_owner_cleanup(self):
        unrelated = subprocess.Popen(self.command("import time;time.sleep(30)"))
        owner = f"""
import sys
sys.path.insert(0,{str(INTEGRATION)!r})
from session_process import Processes
with Processes() as processes:
 assert processes.run({self.command(self.tree(True))!r}, {str(self.root)!r}, 5)==0
 assert processes.children()==[]
"""
        try:
            result = subprocess.run(self.command(owner), stderr=subprocess.PIPE, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertIsNone(unrelated.poll())
        finally:
            unrelated.terminate()
            unrelated.wait(timeout=3)
        self.assert_reaped()

    def test_prior_subreaper_and_signal_handlers_are_restored(self):
        def get_state():
            value = ctypes.c_int()
            Processes.prctl(37, ctypes.byref(value))
            return value.value

        initial = get_state()
        handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
        try:
            for prior in (0, 1):
                Processes.prctl(36, prior)
                with self.assertRaisesRegex(RuntimeError, "intentional"):
                    with Processes():
                        self.assertEqual(get_state(), 1)
                        raise RuntimeError("intentional")
                self.assertEqual(get_state(), prior)
                self.assertEqual({sig: signal.getsignal(sig) for sig in handlers}, handlers)
        finally:
            Processes.prctl(36, initial)


if __name__ == "__main__":
    unittest.main()
