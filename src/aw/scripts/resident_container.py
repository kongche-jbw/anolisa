"""Build an allowlisted image and run credential-free native resident acceptance."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import uuid

from check import AW, REPO, git, sha
from check_native import load
from native_acceptance import Commands, verify_bundle
from resident_contract import BINARIES, suites, verify

SOURCES = (
    "scripts/check.py",
    "scripts/check_native.py",
    "scripts/native_acceptance.py",
    "scripts/native_cleanup.py",
    "scripts/native_qoder.py",
    "scripts/native_reports.py",
    "scripts/resident_contract.py",
    "tests/native-matrix.json",
    "tests/sdk_resident.py",
    "tests/resident_image.py",
    "crates/aw-cli/tests/fixtures/claude-sdk/native_worker.py",
)


def source_files(root: Path) -> list[Path]:
    """Exclude checkout metadata, deployment configuration and generated artifacts."""
    paths = [root / name for name in SOURCES]
    paths.extend(sorted((root / "tests/fixtures/resident").glob("*.py")))
    paths.extend(sorted((root / "tests/fixtures/resident").glob("*.mjs")))
    if any(path.is_symlink() or not path.is_file() for path in paths):
        raise ValueError("resident image requires regular allowlisted source files")
    return paths


def stage(
    root: Path, binaries: Path, context: Path, expected: str, manifest: dict
) -> dict:
    """Give Docker only reviewed public fixtures and the checksummed candidate payload."""
    verify_bundle(binaries, expected)
    selected = suites(manifest)
    record = {}
    for source in source_files(root):
        relative = source.relative_to(root)
        target = context / "source" / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        record[str(relative)] = hashlib.sha256(source.read_bytes()).hexdigest()
    (context / "source/source-sha.txt").write_text(expected + "\n")
    payload = context / "payload"
    payload.mkdir()
    checksums = []
    for line in (binaries / "SHA256SUMS").read_text().splitlines():
        _, name = line.split("  ", 1)
        if name not in BINARIES:
            continue
        shutil.copyfile(binaries / name, payload / name)
        (payload / name).chmod(0o755)
        checksums.append(line)
    (payload / "SHA256SUMS").write_text("\n".join(checksums) + "\n")
    shutil.copyfile(binaries / "source-sha.txt", payload / "source-sha.txt")
    (context / "python-version.txt").write_text(selected["claude-sdk"]["python"] + "\n")
    (context / "requirements.txt").write_text(
        "\n".join(selected["claude-sdk"]["requirements"]) + "\n"
    )
    (context / "pi").mkdir()
    for name in ("package.json", "package-lock.json"):
        shutil.copyfile(
            root / selected["pi"]["npm_fixture"] / name, context / "pi" / name
        )
    return record


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--bin", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    expected = sha(args.candidate_sha)
    if git("rev-parse", "HEAD") != expected:
        raise ValueError("resident image checkout differs from candidate")
    paths = [str(path.relative_to(REPO)) for path in source_files(AW)]
    paths += [
        "src/aw/tests/container/Dockerfile",
        "src/aw/crates/aw-cli/tests/fixtures/pi",
    ]
    if git("status", "--porcelain", "--untracked-files=all", "--", *paths):
        raise ValueError("resident image inputs contain uncommitted changes")
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    owner = uuid.uuid4().hex
    name, tag = "aw-resident-" + owner, "aw-resident:" + owner
    environment = {
        key: value
        for key, value in os.environ.items()
        if key in {"PATH", "HOME", "DOCKER_HOST", "DOCKER_CONTEXT", "DOCKER_CONFIG"}
    }
    commands = Commands(args.output, environment)
    record = {
        "candidate_sha": expected,
        "name": name,
        "tag": tag,
        "ports": [],
        "stop": f"docker stop --time 15 {name}",
        "remove": f"docker rm -f {name}",
        "remove_image": f"docker image rm {tag}",
        "removed": False,
        "image_removed": False,
    }

    def docker(*argv: str) -> str:
        return subprocess.check_output(
            ["docker", *argv],
            env=environment,
            text=True,
            stderr=subprocess.PIPE,
            timeout=30,
        ).strip()

    try:
        with tempfile.TemporaryDirectory(
            prefix="resident-image-", dir=AW / "target"
        ) as temporary:
            context = Path(temporary)
            inputs = stage(AW, args.bin.resolve(), context, expected, load(AW))
            shutil.copyfile(
                REPO / "src/aw/tests/container/Dockerfile", context / "Dockerfile"
            )
            (args.output / "context.json").write_text(
                json.dumps(inputs, indent=2) + "\n"
            )
            commands.run(
                [
                    "docker",
                    "buildx",
                    "build",
                    "--load",
                    "--tag",
                    tag,
                    "--label",
                    "org.opencontainers.image.revision=" + expected,
                    "--label",
                    "aw.resident-owner=" + owner,
                    str(context),
                ],
                REPO,
                1200,
            )
        record["id"] = docker(
            "create",
            "--name",
            name,
            "--init",
            "--network",
            "none",
            "--security-opt",
            "no-new-privileges",
            "--cap-drop",
            "ALL",
            "--pids-limit",
            "512",
            "--memory",
            "4g",
            tag,
        )
        details = json.loads(docker("inspect", name))[0]
        record.update(
            image=details["Image"],
            network=details["HostConfig"]["NetworkMode"],
            mounts=details["Mounts"],
        )
        (args.output / "container.json").write_text(json.dumps(record, indent=2) + "\n")
        try:
            commands.run(["docker", "start", "--attach", name], REPO, 2400)
        finally:
            # The filesystem persists after exit; copy only the inner allowlisted evidence.
            docker("cp", name + ":/evidence/.", str(args.output))
    finally:
        try:
            if docker("ps", "-aq", "--filter", "name=^/" + name + "$"):
                docker("rm", "-f", name)
            record["removed"] = not docker(
                "ps", "-aq", "--filter", "name=^/" + name + "$"
            )
        finally:
            try:
                if docker("image", "ls", "-q", tag):
                    docker("image", "rm", tag)
                record["image_removed"] = not docker("image", "ls", "-q", tag)
            finally:
                (args.output / "container.json").write_text(
                    json.dumps(record, indent=2) + "\n"
                )
    verify(args.output, load(AW), expected)
    print("Resident container passed with complete native evidence and cleanup")


if __name__ == "__main__":

    def interrupted(signum: int, _frame: object) -> None:
        raise InterruptedError(f"received signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    os.umask(0o077)
    main()
