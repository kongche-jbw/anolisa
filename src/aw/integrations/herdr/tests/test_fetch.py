"""Verify pinned downloads never replace mismatching user-owned artifacts."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location(
    "fetch", Path(__file__).parents[1] / "fetch.py"
)
fetch = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fetch)


class FetchTests(unittest.TestCase):
    def test_matching_artifact_is_reused_offline(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "herdr"
            target.write_bytes(b"pinned")
            with patch.object(fetch.urllib.request, "urlopen") as open_url:
                fetch.download("unused", target, hashlib.sha256(b"pinned").hexdigest())
            open_url.assert_not_called()

    def test_existing_mismatch_is_preserved(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "herdr"
            target.write_bytes(b"user artifact")
            with self.assertRaises(ValueError):
                fetch.download("unused", target, "0" * 64)
            self.assertEqual(target.read_bytes(), b"user artifact")

    def test_bad_download_leaves_no_artifact(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "herdr"
            with patch.object(
                fetch.urllib.request, "urlopen", return_value=io.BytesIO(b"wrong")
            ):
                with self.assertRaises(ValueError):
                    fetch.download("unused", target, "0" * 64)
            self.assertEqual(list(Path(temporary).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
