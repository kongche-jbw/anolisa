"""Ownership, reset and failure cleanup checks for the event runner."""

import fcntl
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).resolve().parents[2] / "scripts/ubuntu-experience/demo.py"
SPEC = importlib.util.spec_from_file_location("demo", SCRIPT)
demo = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(demo)


class DemoTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="experience-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "event space"

    def prepare(self):
        with patch.object(demo, "refresh_report"):
            demo.prepare(self.root)

    def test_prepare_refuses_existing_user_directory(self):
        self.root.mkdir()
        marker = self.root / "user-data"
        marker.write_text("keep")
        with self.assertRaises(RuntimeError):
            demo.prepare(self.root)
        self.assertEqual(marker.read_text(), "keep")

    def test_reset_preserves_auth_and_source_but_removes_round_data(self):
        self.prepare()
        auth = self.root / "qoder-config/private"
        auth.write_text("local-test-credential")
        source = self.root / "skills/rpm-package-inspector/SKILL.md"
        before = source.read_bytes()
        output = self.root / "runtime/participant-output"
        output.write_text("old round")
        with patch.object(demo, "refresh_report"):
            demo.reset(self.root)
        self.assertFalse(output.exists())
        self.assertEqual(auth.read_text(), "local-test-credential")
        self.assertEqual(source.read_bytes(), before)
        self.assertTrue((self.root / "runtime/raw/.qoder/skills").is_symlink())

    def test_reset_refuses_active_mount(self):
        self.prepare()
        with patch.object(
            demo, "mounts_under", return_value=[str(self.root / "runtime/mount")]
        ):
            with self.assertRaises(RuntimeError):
                demo.reset(self.root)
        self.assertTrue((self.root / "runtime/raw").is_dir())

    def test_symlink_runtime_is_not_owned(self):
        self.prepare()
        demo.shutil.rmtree(self.root / "runtime")
        (self.root / "runtime").symlink_to(self.temp.name, target_is_directory=True)
        with self.assertRaises(RuntimeError):
            demo.require_owned(self.root)

    def test_modified_source_is_detected(self):
        self.prepare()
        (self.root / "skills/rpm-package-inspector/SKILL.md").write_text("modified")
        with self.assertRaises(RuntimeError):
            demo.source_check(self.root)

    def test_concurrent_reset_is_rejected(self):
        self.prepare()
        with (self.root / ".lock").open("a") as handle:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(self.root), "reset"],
                capture_output=True,
                text=True,
                timeout=10,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Another demo is running", result.stderr)

    def test_qoder_timeout_reaps_owned_child(self):
        self.prepare()
        pid_file = Path(self.temp.name) / "child.pid"
        child_code = (
            "import os,time; from pathlib import Path; "
            f"Path({str(pid_file)!r}).write_text(str(os.getpid())); time.sleep(30)"
        )
        with self.assertRaises(subprocess.TimeoutExpired):
            demo.qoder_process(
                self.root,
                [sys.executable, "-c", child_code],
                self.root,
                {},
                subprocess.DEVNULL,
                0.3,
                None,
            )
        self.assertFalse((self.root / "runtime/qoder-process.json").exists())
        with self.assertRaises(ProcessLookupError):
            os.kill(int(pid_file.read_text()), 0)

    def test_mount_startup_failure_reaps_child(self):
        self.prepare()
        executable = Path(self.temp.name) / "failing-skillfs"
        executable.write_text("#!/bin/sh\nexit 17\n")
        executable.chmod(0o755)
        with self.assertRaisesRegex(RuntimeError, "SkillFS exited"):
            with demo.mounted(self.root, str(executable)):
                self.fail("A failed mount must not reach the body")
        self.assertFalse((self.root / "runtime/mount-process.json").exists())


if __name__ == "__main__":
    unittest.main()
