#!/usr/bin/env python3
"""Fetch an unmodified, digest-pinned Linux Herdr release and its license."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import tempfile
import urllib.request


def download(url: str, target: Path, digest: str) -> None:
    if target.exists():
        if hashlib.sha256(target.read_bytes()).hexdigest() != digest:
            raise ValueError(f"existing artifact digest differs: {target}")
        return
    with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                for chunk in iter(lambda: response.read(1024 * 1024), b""):
                    output.write(chunk)
            output.flush()
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != digest:
                raise ValueError("download digest differs from the committed pin")
            os.link(temporary, target)
        finally:
            temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    pin = json.loads(Path(__file__).with_name("upstream.json").read_text())
    if platform.system() != "Linux" or platform.machine() not in pin["assets"]:
        parser.error("only Linux aarch64 and x86_64 release artifacts are pinned")
    args.destination.mkdir(parents=True, exist_ok=True)
    asset = pin["assets"][platform.machine()]
    binary = args.destination / "herdr"
    download(
        f'{pin["repository"]}/releases/download/{pin["tag"]}/{asset["name"]}',
        binary,
        asset["sha256"],
    )
    download(
        f'https://raw.githubusercontent.com/herdrdev/herdr/{pin["commit"]}/LICENSE',
        args.destination / "LICENSE.herdr",
        pin["license_sha256"],
    )
    binary.chmod(0o755)
    print(binary.resolve())


if __name__ == "__main__":
    main()
