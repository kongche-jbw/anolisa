"""Exercise provider discovery and launcher failure boundaries without an Agent."""

import argparse
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

AW = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("aw_demo", AW / "scripts/demo.py")
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)


class DemoTests(unittest.TestCase):
    def setUp(self):
        (AW / "target").mkdir(exist_ok=True)
        self.temporary = tempfile.TemporaryDirectory(prefix="demo-test-", dir=AW / "target")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        for path in (AW / "providers").glob("*.json"):
            (self.root / path.name).write_bytes(path.read_bytes())

    def test_paths_follow_manifest_location_without_executing(self):
        with patch.object(demo.subprocess, "run", side_effect=AssertionError("must not execute")):
            items = demo.discover(self.root)
        expected = self.root.parent / "target/demo/build/tokenless/debug/tokenless"
        self.assertEqual(items["tokenless"]["program"], str(expected))

    def test_venv_symlink_is_not_resolved(self):
        (self.root / "python").symlink_to(sys.executable)
        path = self.root / "sec-core.json"
        item = json.loads(path.read_text())
        item["program"] = "python"
        path.write_text(json.dumps(item))
        self.assertEqual(demo.discover(self.root)["sec-core"]["program"], str(self.root / "python"))

    def test_missing_duplicate_and_unknown_provider_fail(self):
        path = self.root / "tokenless.json"
        original = path.read_text()
        path.unlink()
        with self.assertRaisesRegex(ValueError, "exactly one"):
            demo.discover(self.root)
        path.write_text(original)
        duplicate = self.root / "duplicate.json"
        duplicate.write_text(original)
        with self.assertRaisesRegex(ValueError, "duplicate"):
            demo.discover(self.root)
        duplicate.write_text(original.replace('"tokenless"', '"unknown"'))
        with self.assertRaisesRegex(ValueError, "unsupported"):
            demo.discover(self.root)

    def test_version_and_native_protocol_fail_before_execution(self):
        path = self.root / "tokenless.json"
        original = json.loads(path.read_text())
        for field, value in (("version", "0.7.0"), ("native_protocol", 1), ("kind", "security")):
            with self.subTest(field=field):
                path.write_text(json.dumps(dict(original, **{field: value})))
                with self.assertRaisesRegex(ValueError, "version/protocol"):
                    demo.discover(self.root)

    def test_consent_checked_before_doctor_or_launch(self):
        with patch.object(demo, "doctor", side_effect=AssertionError("must not execute")):
            with self.assertRaisesRegex(ValueError, "allow-unrecoverable"):
                demo.run(argparse.Namespace(allow_unrecoverable=False))

    def test_failed_setup_step_records_exit_and_log(self):
        with self.assertRaisesRegex(RuntimeError, "exited 23"):
            demo.execute(
                [sys.executable, "-c", "raise SystemExit(23)"], self.root, self.root / "logs", 5
            )
        record = json.loads(next((self.root / "logs").glob("*.json")).read_text())
        self.assertEqual(record["returncode"], 23)
        self.assertTrue(Path(record["log"]).is_file())
        self.assertFalse(Path(f"/proc/{record['pid']}").exists())

    def test_timed_out_setup_step_is_reaped(self):
        with self.assertRaises(subprocess.TimeoutExpired):
            demo.execute(
                [sys.executable, "-c", "import time; time.sleep(60)"],
                self.root,
                self.root / "logs",
                1,
            )
        record = json.loads(next((self.root / "logs").glob("*.json")).read_text())
        self.assertLess(record["returncode"], 0)
        self.assertFalse(Path(f"/proc/{record['pid']}").exists())


if __name__ == "__main__":
    unittest.main()
