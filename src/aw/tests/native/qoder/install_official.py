#!/usr/bin/env python3
"""Extract the official checksummed CLI without changing user configuration."""

import argparse
import hashlib
import json
from pathlib import Path
import platform
import tarfile
import urllib.request


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    root = args.root.resolve()
    downloads = root / "downloads"
    downloads.mkdir(parents=True, exist_ok=True)
    manifest_url = "https://qoder-ide.oss-accelerate.aliyuncs.com/qodercli/channels/manifest.json"
    with urllib.request.urlopen(manifest_url, timeout=30) as response:
        manifest_data = response.read()
    (downloads / "manifest.json").write_bytes(manifest_data)
    manifest = json.loads(manifest_data)
    entries = manifest["files"]
    architecture = {"aarch64": "arm64", "x86_64": "amd64-baseline"}[platform.machine()]
    artifact = next(item for item in entries
                    if item["os"] == "linux" and item["arch"] == architecture)
    archive = downloads / "qodercli.tar.gz"
    with urllib.request.urlopen(artifact["url"], timeout=60) as response, archive.open("wb") as output:
        while block := response.read(1024 * 1024):
            output.write(block)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != artifact["sha256"]:
        raise ValueError("Official Qoder archive checksum mismatch")
    binary_dir = root / "bin"
    binary_dir.mkdir(exist_ok=True)
    binary = binary_dir / "qodercli"
    with tarfile.open(archive) as bundle:
        member = next(item for item in bundle.getmembers() if item.name.lstrip("./") == "qodercli")
        source = bundle.extractfile(member)
        if source is None:
            raise ValueError("Official archive has no CLI executable")
        binary.write_bytes(source.read())
    binary.chmod(0o755)
    receipt = {"manifest_url": manifest_url, "version": manifest["latest"],
               "archive_url": artifact["url"], "archive_sha256": digest,
               "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
               "binary": str(binary)}
    (root / "installation.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt))


if __name__ == "__main__":
    main()
