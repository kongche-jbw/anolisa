"""Exercise offline reuse, atomic publication, and bounded fetch cleanup."""

from contextlib import redirect_stderr, redirect_stdout
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import urllib.error


SCRIPT = Path(__file__).resolve().parents[1] / "fetch.py"
SPEC = importlib.util.spec_from_file_location("fetch", SCRIPT)
fetch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fetch)
TARGET = SCRIPT.parents[2] / "target"
BINARY = b"pinned binary"
LICENSE = b"pinned license"
PIN = {
    "repository": "https://example.invalid/herdr",
    "tag": "v0.9.0",
    "commit": "fixture-commit",
    "license_sha256": hashlib.sha256(LICENSE).hexdigest(),
    "assets": {
        "x86_64": {
            "name": "herdr-linux-x86_64",
            "sha256": hashlib.sha256(BINARY).hexdigest(),
        }
    },
}


class FetchTests(unittest.TestCase):
    def setUp(self) -> None:
        TARGET.mkdir(exist_ok=True)
        temporary = tempfile.TemporaryDirectory(prefix="herdr-fetch-test-", dir=TARGET)
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.destination = self.root / "bundle"
        system = patch.object(fetch.platform, "system", return_value="Linux")
        machine = patch.object(fetch.platform, "machine", return_value="x86_64")
        system.start()
        machine.start()
        self.addCleanup(system.stop)
        self.addCleanup(machine.stop)

    def bundle(self) -> None:
        self.destination.mkdir()
        (self.destination / "herdr").write_bytes(BINARY)
        (self.destination / "herdr").chmod(0o755)
        (self.destination / "LICENSE.herdr").write_bytes(LICENSE)

    def fetch(self, responses: list) -> Path:
        with patch.object(fetch.urllib.request, "urlopen", side_effect=responses):
            return fetch.fetch_bundle(self.destination, PIN)

    def test_matching_bundle_is_reused_offline_without_mode_changes(self) -> None:
        self.bundle()
        binary = self.destination / "herdr"
        binary.chmod(0o500)
        (self.destination / "notes").write_bytes(b"user notes")
        paths = [self.destination, *self.destination.iterdir()]
        before = {path: (path.stat().st_mode, path.stat().st_mtime_ns) for path in paths}
        with patch.object(fetch.urllib.request, "urlopen") as open_url:
            self.assertEqual(fetch.fetch_bundle(self.destination, PIN), binary)
        open_url.assert_not_called()
        after = {path: (path.stat().st_mode, path.stat().st_mtime_ns) for path in paths}
        self.assertEqual(before, after)
        self.assertEqual((self.destination / "notes").read_bytes(), b"user notes")

    def test_non_executable_bundle_is_refused_without_permission_repair(self) -> None:
        self.bundle()
        binary = self.destination / "herdr"
        binary.chmod(0o400)
        before = binary.stat()
        with self.assertRaisesRegex(ValueError, "not executable"):
            self.fetch([])
        after = binary.stat()
        self.assertEqual(after.st_mode, before.st_mode)
        self.assertEqual(after.st_mtime_ns, before.st_mtime_ns)
        self.assertEqual(binary.read_bytes(), BINARY)

    def test_existing_mismatch_is_preserved(self) -> None:
        self.bundle()
        (self.destination / "herdr").write_bytes(b"user artifact")
        with self.assertRaises(ValueError):
            self.fetch([])
        self.assertEqual((self.destination / "herdr").read_bytes(), b"user artifact")
        self.assertEqual((self.destination / "LICENSE.herdr").read_bytes(), LICENSE)

    def test_existing_license_mismatch_is_preserved(self) -> None:
        self.bundle()
        (self.destination / "LICENSE.herdr").write_bytes(b"user license")
        with self.assertRaises(ValueError):
            self.fetch([])
        self.assertEqual((self.destination / "LICENSE.herdr").read_bytes(), b"user license")

    def test_incomplete_bundle_is_not_repaired(self) -> None:
        self.destination.mkdir()
        (self.destination / "herdr").write_bytes(BINARY)
        with self.assertRaises(FileNotFoundError):
            self.fetch([])
        self.assertEqual(list(self.destination.iterdir()), [self.destination / "herdr"])

    def test_existing_user_directory_is_preserved(self) -> None:
        self.destination.mkdir()
        (self.destination / "notes").write_bytes(b"keep")
        with self.assertRaises(FileNotFoundError):
            self.fetch([])
        self.assertEqual((self.destination / "notes").read_bytes(), b"keep")

    def test_existing_user_file_is_preserved(self) -> None:
        self.destination.write_bytes(b"keep")
        with self.assertRaises(ValueError):
            self.fetch([])
        self.assertEqual(self.destination.read_bytes(), b"keep")

    def test_symlink_destination_is_refused(self) -> None:
        actual = self.root / "actual"
        actual.mkdir()
        self.destination.symlink_to(actual, target_is_directory=True)
        with self.assertRaises(ValueError):
            self.fetch([])
        self.assertTrue(self.destination.is_symlink())
        self.assertEqual(list(actual.iterdir()), [])

    def test_broken_symlink_destination_is_refused(self) -> None:
        self.destination.symlink_to(self.root / "absent")
        with self.assertRaises(ValueError):
            self.fetch([])
        self.assertTrue(self.destination.is_symlink())

    def test_symlink_artifact_is_refused(self) -> None:
        self.bundle()
        binary = self.destination / "herdr"
        binary.unlink()
        external = self.root / "external"
        external.write_bytes(BINARY)
        binary.symlink_to(external)
        with self.assertRaises(OSError):
            self.fetch([])
        self.assertTrue(binary.is_symlink())
        self.assertEqual(external.read_bytes(), BINARY)

    def test_fifo_artifact_is_refused_without_blocking(self) -> None:
        self.destination.mkdir()
        os.mkfifo(self.destination / "herdr")
        with self.assertRaises(ValueError):
            self.fetch([])

    def test_complete_verified_bundle_is_published(self) -> None:
        binary = self.fetch([io.BytesIO(BINARY), io.BytesIO(LICENSE)])
        self.assertEqual(binary.read_bytes(), BINARY)
        self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
        self.assertEqual((self.destination / "LICENSE.herdr").read_bytes(), LICENSE)
        self.assertEqual(list(self.root.iterdir()), [self.destination])

    def test_bad_download_leaves_no_bundle_or_staging(self) -> None:
        with self.assertRaises(ValueError):
            self.fetch([io.BytesIO(b"wrong")])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_interrupt_after_publication_preserves_complete_bundle(self) -> None:
        publish = fetch.publish

        def interrupt_after_publish(staging: Path, destination: Path) -> None:
            publish(staging, destination)
            os.kill(os.getpid(), signal.SIGTERM)

        with fetch.deadline(2), patch.object(fetch, "publish", interrupt_after_publish):
            with self.assertRaises(InterruptedError):
                self.fetch([io.BytesIO(BINARY), io.BytesIO(LICENSE)])
        self.assertEqual(list(self.root.iterdir()), [self.destination])
        self.assertEqual((self.destination / "herdr").read_bytes(), BINARY)
        self.assertEqual((self.destination / "LICENSE.herdr").read_bytes(), LICENSE)

    def test_output_error_after_publication_preserves_complete_bundle(self) -> None:
        class BrokenOutput(io.StringIO):
            def write(self, text: str) -> int:
                raise BrokenPipeError("output closed")

        stderr = io.StringIO()
        with patch.object(fetch.json, "loads", return_value=PIN):
            with patch.object(
                fetch.urllib.request, "urlopen", side_effect=[io.BytesIO(BINARY), io.BytesIO(LICENSE)]
            ):
                with redirect_stdout(BrokenOutput()), redirect_stderr(stderr):
                    status = fetch.main([str(self.destination)])
        self.assertEqual(status, 1)
        self.assertIn("output closed", stderr.getvalue())
        self.assertEqual(list(self.root.iterdir()), [self.destination])
        self.assertEqual((self.destination / "herdr").read_bytes(), BINARY)
        self.assertEqual((self.destination / "LICENSE.herdr").read_bytes(), LICENSE)

    def test_license_digest_failure_removes_staged_binary(self) -> None:
        with self.assertRaises(ValueError):
            self.fetch([io.BytesIO(BINARY), io.BytesIO(b"wrong license")])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_network_failures_leave_no_bundle_or_staging(self) -> None:
        for responses in (
            [urllib.error.URLError("offline")],
            [io.BytesIO(BINARY), urllib.error.URLError("offline")],
        ):
            with self.subTest(stage=len(responses)):
                with self.assertRaises(OSError):
                    self.fetch(responses)
                self.assertEqual(list(self.root.iterdir()), [])

    def test_empty_destination_created_during_fetch_is_not_replaced(self) -> None:
        publish = fetch.publish

        def collide(staging: Path, destination: Path) -> None:
            destination.mkdir()
            original_inode = destination.stat().st_ino
            with self.assertRaises(FileExistsError):
                publish(staging, destination)
            self.assertEqual(destination.stat().st_ino, original_inode)
            raise FileExistsError("collision")

        with patch.object(fetch, "publish", side_effect=collide):
            with self.assertRaises(FileExistsError):
                self.fetch([io.BytesIO(BINARY), io.BytesIO(LICENSE)])
        self.assertEqual(list(self.root.iterdir()), [self.destination])
        self.assertEqual(list(self.destination.iterdir()), [])

    def test_unsupported_architecture_does_not_create_destination(self) -> None:
        with patch.object(fetch.platform, "machine", return_value="riscv64"):
            with self.assertRaises(ValueError):
                self.fetch([])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_unsupported_platform_does_not_create_destination(self) -> None:
        with patch.object(fetch.platform, "system", return_value="Darwin"):
            with self.assertRaises(ValueError):
                self.fetch([])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_download_byte_limit_reclaims_staging(self) -> None:
        with patch.object(fetch, "MAX_ARTIFACT_BYTES", 3):
            with self.assertRaisesRegex(ValueError, "byte limit"):
                self.fetch([io.BytesIO(BINARY)])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_existing_byte_limit_does_not_modify_bundle(self) -> None:
        self.bundle()
        with patch.object(fetch, "MAX_ARTIFACT_BYTES", 3):
            with self.assertRaisesRegex(ValueError, "byte limit"):
                self.fetch([])
        self.assertEqual((self.destination / "herdr").read_bytes(), BINARY)

    def test_missing_parent_is_not_created(self) -> None:
        self.destination = self.root / "missing-parent" / "bundle"
        with self.assertRaises(FileNotFoundError):
            self.fetch([])
        self.assertEqual(list(self.root.iterdir()), [])

    def test_cli_error_is_concise(self) -> None:
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(fetch.platform, "machine", return_value="riscv64"):
            with redirect_stdout(stdout), redirect_stderr(stderr):
                status = fetch.main([str(self.destination)])
        self.assertEqual(status, 1)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("Herdr fetch failed: only Linux", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_truncated_http_response_is_reported_and_reclaimed(self) -> None:
        class TruncatedResponse(io.BytesIO):
            def read(self, size: int = -1) -> bytes:
                raise fetch.http.client.IncompleteRead(b"partial", 100)

        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.object(fetch.urllib.request, "urlopen", return_value=TruncatedResponse()):
            with redirect_stdout(stdout), redirect_stderr(stderr):
                status = fetch.main([str(self.destination)])
        self.assertEqual(status, 1)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("Herdr fetch failed:", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())
        self.assertEqual(list(self.root.iterdir()), [])

    def test_signal_and_deadline_cleanup_in_real_subprocess(self) -> None:
        for termination in ("term", "interrupt", "timeout"):
            with self.subTest(termination=termination):
                code = """
import importlib.util, os, signal, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location('fetch', sys.argv[1])
fetch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch)
fetch.platform.system = lambda: 'Linux'
fetch.platform.machine = lambda: 'x86_64'
def stalled_download(url, target, digest):
    target.write_bytes(b'partial')
    if sys.argv[3] == 'timeout':
        signal.pause()
    else:
        os.kill(os.getpid(), signal.SIGTERM if sys.argv[3] == 'term' else signal.SIGINT)
fetch.download = stalled_download
sys.exit(fetch.main([sys.argv[2], '--timeout', '0.05']))
"""
                result = subprocess.run(
                    [
                        sys.executable, "-B", "-c", code, str(SCRIPT),
                        str(self.destination), termination,
                    ],
                    capture_output=True,
                    text=True,
                    timeout=5,
                    check=False,
                )
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn("Herdr fetch failed:", result.stderr)
                self.assertNotIn("Traceback", result.stderr)
                self.assertEqual(list(self.root.iterdir()), [])

if __name__ == "__main__":
    unittest.main()
