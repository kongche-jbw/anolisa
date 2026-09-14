#!/usr/bin/env python3
"""Fetch a digest-pinned Linux Herdr bundle without activating an Agent."""

import argparse
from contextlib import contextmanager
import ctypes
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import signal
import stat
import sys
import tempfile
from typing import Iterator
import urllib.request


MAX_ARTIFACT_BYTES = 128 * 1024 * 1024
CHUNK_BYTES = 1024 * 1024


def verify(target: Path, digest: str) -> None:
    """Read regular files only; never repair an existing bundle in place."""
    descriptor = os.open(target, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as source:
        if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
            raise ValueError(f"artifact is not a regular file: {target}")
        actual = hashlib.sha256()
        size = 0
        for chunk in iter(lambda: source.read(CHUNK_BYTES), b""):
            size += len(chunk)
            if size > MAX_ARTIFACT_BYTES:
                raise ValueError(f"artifact exceeds byte limit: {target}")
            actual.update(chunk)
        if actual.hexdigest() != digest:
            raise ValueError(f"artifact digest differs from committed pin: {target}")


def download(url: str, target: Path, digest: str) -> None:
    """Download into a private staging directory and verify before publication."""
    with target.open("xb") as output:
        with urllib.request.urlopen(url, timeout=30) as response:
            size = 0
            for chunk in iter(lambda: response.read(CHUNK_BYTES), b""):
                size += len(chunk)
                if size > MAX_ARTIFACT_BYTES:
                    raise ValueError("download exceeds artifact byte limit")
                output.write(chunk)
    verify(target, digest)


def publish(staging: Path, destination: Path) -> None:
    """Atomically publish both files without replacing a raced-in destination."""
    # os.rename can overwrite an empty user directory; Linux NOREPLACE cannot.
    libc = ctypes.CDLL(None, use_errno=True)
    try:
        rename = libc.renameat2
    except AttributeError as error:
        raise OSError("Linux renameat2 is required for safe publication") from error
    rename.argtypes = [
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.c_uint,
    ]
    rename.restype = ctypes.c_int
    if rename(-100, os.fsencode(staging), -100, os.fsencode(destination), 1) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code), str(destination))


def fetch_bundle(destination: Path, pin: dict) -> Path:
    """Reuse a complete executable bundle, or publish a newly verified bundle.

    Publication commits both files atomically. A later interrupt or output error
    leaves that complete bundle in place; committed bundles are never rolled back.
    """
    machine = platform.machine()
    if platform.system() != "Linux" or machine not in pin["assets"]:
        raise ValueError("only Linux aarch64 and x86_64 release artifacts are pinned")
    destination = destination.absolute()
    asset = pin["assets"][machine]
    if os.path.lexists(destination):
        if not stat.S_ISDIR(destination.lstat().st_mode):
            raise ValueError(f"destination is not a regular directory: {destination}")
        verify(destination / "herdr", asset["sha256"])
        verify(destination / "LICENSE.herdr", pin["license_sha256"])
        if not os.access(destination / "herdr", os.X_OK):
            raise ValueError(f"existing Herdr binary is not executable: {destination / 'herdr'}")
        return destination / "herdr"

    with tempfile.TemporaryDirectory(prefix=".herdr-fetch-", dir=destination.parent) as raw:
        staging = Path(raw)
        download(
            f'{pin["repository"]}/releases/download/{pin["tag"]}/{asset["name"]}',
            staging / "herdr",
            asset["sha256"],
        )
        download(
            f'https://raw.githubusercontent.com/herdrdev/herdr/{pin["commit"]}/LICENSE',
            staging / "LICENSE.herdr",
            pin["license_sha256"],
        )
        (staging / "herdr").chmod(0o755)
        publish(staging, destination)
    return destination / "herdr"


@contextmanager
def deadline(seconds: float) -> Iterator[None]:
    """Turn CLI termination and timeout into exceptions so staging is reclaimed."""
    def interrupt(signum: int, _frame: object) -> None:
        if signum == signal.SIGALRM:
            raise TimeoutError("Herdr fetch deadline exceeded")
        raise InterruptedError(f"Herdr fetch interrupted by signal {signum}")

    previous = {}
    try:
        for signum in (signal.SIGALRM, signal.SIGINT, signal.SIGTERM):
            previous[signum] = signal.signal(signum, interrupt)
        signal.setitimer(signal.ITIMER_REAL, seconds)
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path, help="bundle path; parent must exist")
    parser.add_argument("--timeout", type=float, default=120, help="overall seconds (0–600)")
    args = parser.parse_args(argv)
    if not 0 < args.timeout <= 600:
        parser.error("--timeout must be greater than zero and at most 600 seconds")
    try:
        with deadline(args.timeout):
            pin = json.loads(Path(__file__).with_name("upstream.json").read_text())
            print(fetch_bundle(args.destination, pin))
        return 0
    except (OSError, ValueError, http.client.HTTPException) as error:
        print(f"Herdr fetch failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
