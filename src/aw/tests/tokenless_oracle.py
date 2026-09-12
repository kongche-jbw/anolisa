#!/usr/bin/env python3
"""Compare an explicitly supplied native CLI with reviewed protocol fixtures.

This acceptance check runs Tokenless itself. Normal AW CI runs only the oracle's
self-tests and replays the frozen responses through the Rust Host tests.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest

AW = Path(__file__).resolve().parents[1]
FIXTURE = AW / "tests/fixtures/tokenless-native.json"


def cases(document: dict) -> list:
    result = document.get("cases", [])
    if not result or len({case["name"] for case in result}) != len(result):
        raise ValueError("native fixture cases are empty or duplicated")
    if document["native_cli_version"] != "0.8.1" or document["protocol_version"] != 2:
        raise ValueError("native fixture version is unsupported")
    return result


def compare(case: dict, returncode: int, stdout: bytes) -> None:
    if returncode != case["exit_code"] or json.loads(stdout) != case["response"]:
        raise ValueError(f"{case['name']}: native behavior changed; review the mapping")


def invoke(binary: Path, arguments: list, data: bytes, root: Path) -> tuple:
    environment = {
        "TOKENLESS_STATS_ENABLED": "0",
        "TOKENLESS_SLS_ENABLED": "0",
        "TOKENLESS_COMPRESSION_ENABLED": "1",
        "TOKENLESS_DATA_DIR": str(root / "native-state"),
    }
    # File-backed streams keep an unexpected native response out of memory.
    # This fixed CLI operation creates no descendants. Kill only an unreaped
    # child group on failure; never signal a PID after communicate reaped it.
    with tempfile.TemporaryFile(dir=root) as output, tempfile.TemporaryFile(dir=root) as error:
        child = subprocess.Popen(
            [str(binary), *arguments], stdin=subprocess.PIPE, stdout=output,
            stderr=error, cwd=root, env=environment, start_new_session=True,
        )
        try:
            child.communicate(data, timeout=10)
            output.seek(0)
            content = output.read(4 * 1024 * 1024 + 1)
            if len(content) > 4 * 1024 * 1024:
                raise ValueError("native response exceeds acceptance limit")
            return child.returncode, content
        finally:
            if child.returncode is None:
                try:
                    os.killpg(child.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            child.wait(timeout=2)


def check(binary: Path) -> None:
    document = json.loads(FIXTURE.read_text(encoding="utf-8"))
    vectors = cases(document)
    (AW / "target").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="tokenless-oracle-", dir=AW / "target") as tmp:
        root = Path(tmp)
        code, version = invoke(binary, ["--version"], b"", root)
        if code != 0 or version != b"tokenless 0.8.1\n":
            raise ValueError("native CLI version is unsupported")
        for case in vectors:
            data = json.dumps(case["request"], ensure_ascii=False).encode("utf-8")
            code, output = invoke(binary, ["compress"], data, root)
            compare(case, code, output)
            if (root / "native-state").exists():
                raise ValueError("stateless native profile wrote persistent data")
            print(f"{case['name']}: native result matches", flush=True)
    print(f"binary SHA-256: {hashlib.sha256(binary.read_bytes()).hexdigest()}")


class OracleTests(unittest.TestCase):
    def test_empty_or_duplicate_vectors_fail(self) -> None:
        document = json.loads(FIXTURE.read_text(encoding="utf-8"))
        for invalid in ([], [document["cases"][0]] * 2):
            with self.assertRaises(ValueError):
                cases({**document, "cases": invalid})

    def test_native_drift_and_failed_exit_fail(self) -> None:
        case = cases(json.loads(FIXTURE.read_text(encoding="utf-8")))[0]
        compare(case, case["exit_code"], json.dumps(case["response"]).encode())
        for code, value in ((1, case["response"]), (0, {})):
            with self.assertRaises(ValueError):
                compare(case, code, json.dumps(value).encode())


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--binary", type=Path, help="Prebuilt native Tokenless CLI")
    mode.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        unittest.main(argv=[__file__])
    else:
        check(args.binary.resolve(strict=True))
