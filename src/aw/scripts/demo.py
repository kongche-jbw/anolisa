#!/usr/bin/env python3
"""Set up and demonstrate the pinned Qoder / SecCore / Tokenless / Herdr flow."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import signal
import subprocess
import sys
import time
import uuid

AW = Path(__file__).resolve().parents[1]
DATA = AW / "target" / "demo"
SEC_COMMIT = "5ebfc0b3905fa2f5f74aff2da4aec2b3be639647"
SEC_REPOSITORY = "https://github.com/kongche-jbw/anolisa.git"
SEC_PROJECT = DATA / "sec-core/src/agent-sec-core/agent-sec-cli"
EXPECTED = {"sec-core": ("security", "0.11.0", 1), "tokenless": ("projection", "0.8.0", 2)}


def read_json(path: Path) -> dict:
    return json.loads(path.read_text())


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def discover(directory: Path) -> dict:
    """Resolve only explicitly selected manifests; discovery never executes a provider."""
    providers = {}
    for path in sorted(directory.glob("*.json")):
        item = read_json(path)
        identity = item.get("id")
        if identity not in EXPECTED or identity in providers:
            raise ValueError(f"unsupported or duplicate provider in {path}")
        if tuple(item.get(k) for k in ("kind", "version", "native_protocol")) != EXPECTED[identity]:
            raise ValueError(f"unsupported provider version/protocol in {path}")
        for field in ("program", "source") if identity == "sec-core" else ("program",):
            value = item.get(field)
            if not isinstance(value, str) or not value or "\x00" in value:
                raise ValueError(f"missing or invalid {field} in {path}")
            # Keep the venv executable path: resolving its symlink loses Python's venv.
            item[field] = os.path.abspath(path.parent / value)
        providers[identity] = item
    if providers.keys() != EXPECTED.keys():
        raise ValueError(
            f"{directory} must contain exactly one sec-core and one tokenless manifest"
        )
    return providers


def probe(command: list[str], **kwargs: object) -> str:
    return subprocess.run(
        command, check=True, capture_output=True, text=True, timeout=20, **kwargs
    ).stdout.strip()


def execute(command: list[str], directory: Path, log_dir: Path, timeout: int) -> None:
    """Record and bound each owned process group, including interrupted setup steps."""
    log_dir.mkdir(parents=True, exist_ok=True)
    record_path = log_dir / f"step-{time.time_ns()}.json"
    log_path = record_path.with_suffix(".log")
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    env.update(UV_CACHE_DIR=str(DATA / "uv-cache"), UV_PYTHON_INSTALL_DIR=str(DATA / "python"))
    record = {
        "command": command,
        "cwd": str(directory),
        "log": str(log_path),
        "timeout_seconds": timeout,
    }
    write_json(record_path, record)
    print(f"Running: {shlex.join(command)}\nLog: {log_path}", flush=True)
    with log_path.open("w") as log:
        process = subprocess.Popen(
            command, cwd=directory, env=env, stdout=log, stderr=log, start_new_session=True
        )
        record.update(pid=process.pid, stop_command=f"kill -TERM -- -{process.pid}")
        write_json(record_path, record)
        try:
            code = process.wait(timeout=timeout)
            if code:
                raise RuntimeError(f"command exited {code}; inspect {log_path}")
        finally:
            # Children can outlive their leader after a timeout or failed build.
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                try:
                    os.killpg(process.pid, 0)
                except ProcessLookupError:
                    break
                time.sleep(0.1)
            else:
                os.killpg(process.pid, signal.SIGKILL)
            record["returncode"] = process.returncode
            write_json(record_path, record)


def setup() -> None:
    for name in ("git", "cargo", "uv", "cc"):
        if not shutil.which(name):
            raise ValueError(f"missing {name}; see the AW demo guide prerequisites")
    DATA.mkdir(parents=True, exist_ok=True)
    logs = DATA / "setup-logs"
    checkout = DATA / "sec-core"
    if not checkout.exists():
        execute(["git", "init", str(checkout)], AW, logs, 30)
    if not (SEC_PROJECT / "pyproject.toml").exists():
        execute(
            ["git", "-C", str(checkout), "fetch", "--depth=1", SEC_REPOSITORY, SEC_COMMIT],
            AW,
            logs,
            180,
        )
        execute(
            [
                "git",
                "-C",
                str(checkout),
                "sparse-checkout",
                "set",
                "src/agent-sec-core/agent-sec-cli",
            ],
            AW,
            logs,
            30,
        )
        execute(["git", "-C", str(checkout), "checkout", "--detach", SEC_COMMIT], AW, logs, 30)
    if probe(["git", "-C", str(checkout), "rev-parse", "HEAD"]) != SEC_COMMIT:
        raise ValueError(f"provider checkout differs from pin: {checkout}")
    if probe(["git", "-C", str(checkout), "status", "--porcelain", "--untracked-files=no"]):
        raise ValueError(f"provider checkout has edits: {checkout}")
    execute(
        [
            "uv",
            "sync",
            "--project",
            str(SEC_PROJECT),
            "--python",
            "3.11.6",
            "--frozen",
            "--no-dev",
            "--no-install-project",
        ],
        AW,
        logs,
        600,
    )
    for component, package, target in (
        (AW.parent / "tokenless", "tokenless-cli", "tokenless"),
        (AW, "aw-hook-cli", "aw"),
        (AW.parent / "cosh-ng", "cosh-shell", "cosh"),
    ):
        execute(
            [
                "cargo",
                "build",
                "--manifest-path",
                str(component / "Cargo.toml"),
                "-p",
                package,
                "--bins",
                "--locked",
                "--target-dir",
                str(DATA / "build" / target),
            ],
            AW,
            logs,
            1200,
        )
    execute(
        [sys.executable, str(AW / "integrations/herdr/fetch.py"), str(DATA / "herdr")],
        AW,
        logs,
        180,
    )
    doctor(AW / "providers", agent=False)
    print("Setup complete. Run doctor, then run --allow-unrecoverable.")


def doctor(directory: Path, agent: bool = True) -> dict:
    providers = discover(directory)
    for item in providers.values():
        if not os.access(item["program"], os.X_OK):
            raise ValueError(f"provider executable missing: {item['program']}; run setup")
    sec = providers["sec-core"]
    source = Path(sec["source"])
    if not (source / "agent_sec_cli/aw_provider/runner.py").is_file():
        raise ValueError(f"SecCore native provider missing: {source}")
    check = (
        "import sys; from agent_sec_cli.aw_provider.runner import run_provider; "
        "from agent_sec_cli.aw_provider.protocol import PROTOCOL_VERSION; "
        "import tomllib; from pathlib import Path; "
        "assert sys.version_info[:3] == (3,11,6); assert PROTOCOL_VERSION == 1; "
        "assert tomllib.loads(Path(sys.argv[1]).read_text())['project']['version'] == '0.11.0'"
    )
    probe(
        [sec["program"], "-P", "-c", check, str(source.parent / "pyproject.toml")],
        env={"PYTHONPATH": str(source), "PYTHONDONTWRITEBYTECODE": "1"},
    )
    if probe([providers["tokenless"]["program"], "--version"]) != "tokenless 0.8.0":
        raise ValueError("Tokenless executable must be version 0.8.0")
    for name in ("aw-hook-cli", "aw-view-cli", "aw-adoption-cli"):
        if not os.access(DATA / "build/aw/debug" / name, os.X_OK):
            raise ValueError(f"{name} missing; run setup")
    pin = read_json(AW / "integrations/herdr/upstream.json")
    if (
        hashlib.sha256((DATA / "herdr/herdr").read_bytes()).hexdigest()
        != pin["assets"][platform.machine()]["sha256"]
    ):
        raise ValueError("Herdr digest differs from the pinned release")
    if agent:
        if os.environ.get("QODER_CONFIG_DIR"):
            raise ValueError("this demo uses ~/.qoder; run without QODER_CONFIG_DIR")
        qoder = shutil.which("qodercli")
        if not qoder:
            raise ValueError("qodercli missing; install Qoder CLI 1.1.47 and run qodercli login")
        if probe([qoder, "--version"]) != "1.1.47":
            raise ValueError("this demo requires the validated Qoder CLI 1.1.47")
        settings = Path.home() / ".qoder/settings.json"
        if not settings.is_file():
            raise ValueError(
                "Qoder settings missing; run qodercli login and open qodercli once before doctor"
            )
        if read_json(settings).get("hooks") or json.loads(
            probe([qoder, "plugins", "list", "--json"])
        ):
            raise ValueError(
                "Qoder user hooks/plugins are installed; use a separate demo OS account, without changing existing integrations"
            )
        if json.loads(probe([qoder, "status", "-o", "json"])).get("logged_in") is not True:
            raise ValueError("Qoder is not logged in; run qodercli login")
        print("Qoder configuration and login ready. Model access is verified by the live run.")
    for item in providers.values():
        print(
            f"Provider installed (not a call): {item['id']} {item['version']} (native v{item['native_protocol']}) -> {item['program']}"
        )
    return providers


def run(args: argparse.Namespace) -> None:
    if not args.allow_unrecoverable:
        raise ValueError("run requires --allow-unrecoverable to replace the synthetic tool output")
    if not args.headless and not sys.stdout.isatty():
        raise ValueError("live display requires a terminal; use --headless for captured acceptance")
    if args.case != "compressed" and not args.headless:
        raise ValueError(
            "no-gain/provider-failure use --headless; live Herdr acceptance expects savings"
        )
    providers = doctor(args.provider_dir)
    root = DATA / "runs" / (time.strftime("%Y%m%d-%H%M%S-") + uuid.uuid4().hex[:8])
    root.mkdir(parents=True, mode=0o700)
    sec, tokenless = providers["sec-core"], providers["tokenless"]
    command = [
        sys.executable,
        str(AW / "tests/tokenless_smoke.py"),
        "--case",
        args.case,
        "--provider-python",
        sec["program"],
        "--provider-source",
        sec["source"],
        "--provider-version",
        sec["version"],
        "--tokenless-bin",
        tokenless["program"],
        "--tokenless-version",
        tokenless["version"],
        "--output-dir",
        str(root / "agent"),
    ]
    for name in ("hook", "view", "adoption"):
        command.extend([f"--{name}-bin", str(DATA / "build/aw/debug" / f"aw-{name}-cli")])
    write_json(root / "providers.json", providers)
    write_json(root / "command.json", command)
    print(f"Evidence: {root}", flush=True)
    if args.case == "compressed":
        command = [
            sys.executable,
            str(AW / "integrations/herdr/live_smoke.py"),
            str(DATA / "herdr/herdr"),
            "--output",
            str(root / "herdr"),
            "--command-json",
            str(root / "command.json"),
            "--hold-seconds",
            str(args.hold_seconds),
        ]
        if not args.headless:
            command.append("--display")
    # Inherit the terminal for live presentation. The runner owns and reaps its children.
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    process = subprocess.Popen(command, cwd=AW, env=env, start_new_session=True)
    launch = {
        "command": command,
        "cwd": str(AW),
        "pid": process.pid,
        "stop_command": f"kill -TERM {process.pid}",
        "timeout_seconds": 240 + args.hold_seconds,
    }
    write_json(root / "launcher.json", launch)
    try:
        code = process.wait(timeout=240 + args.hold_seconds)
    finally:
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        signal.signal(signal.SIGINT, signal.SIG_IGN)
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=30)
        launch["returncode"] = process.returncode
        write_json(root / "launcher.json", launch)
    if code:
        raise RuntimeError(f"demo exited {code}; inspect {root}")
    result = read_json(root / "agent/result.json")
    projection = next(p for p in result["view"]["providers"] if p["kind"] == "projection")
    print(
        f"Verified local history: {projection['adopted']} adoption(s), {projection['saved_bytes']} B saved."
    )
    print(f"Demo passed. Evidence: {root}\nRemove this run: rm -rf -- {shlex.quote(str(root))}")


def interrupted(_signal: int, _frame: object) -> None:
    raise KeyboardInterrupt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--provider-dir",
        type=Path,
        default=AW / "providers",
        help="trusted manifests; relative program/source paths resolve against each manifest",
    )
    sub = parser.add_subparsers(dest="action", required=True)
    sub.add_parser("setup", help="fetch pinned SecCore/Herdr and build the demo locally")
    sub.add_parser("providers", help="list discovered manifests without executing them")
    sub.add_parser("doctor", help="check providers, binaries, and Qoder configuration")
    launch = sub.add_parser("run", help="run one synthetic real-Qoder demonstration")
    launch.add_argument("--allow-unrecoverable", action="store_true")
    launch.add_argument("--headless", action="store_true")
    launch.add_argument(
        "--case", choices=["compressed", "no-gain", "provider-failure"], default="compressed"
    )
    launch.add_argument(
        "--hold-seconds", type=int, choices=range(1, 121), default=20, metavar="1..120"
    )
    args = parser.parse_args()
    signal.signal(signal.SIGTERM, interrupted)
    try:
        if platform.system() != "Linux" or platform.machine() not in ("aarch64", "x86_64"):
            raise ValueError("the live demo requires Linux aarch64 or x86_64")
        if args.action == "setup":
            setup()
        elif args.action == "providers":
            print(json.dumps(discover(args.provider_dir), indent=2))
        elif args.action == "doctor":
            doctor(args.provider_dir)
        else:
            run(args)
    except (
        OSError,
        ValueError,
        RuntimeError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ) as error:
        print(f"AW demo failed: {error or 'interrupted'}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
