#!/usr/bin/env python3
"""Run two short Ubuntu/Qoder experiences with owned, resettable state."""

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import time

HERE = Path(__file__).resolve().parent
MARKER = "anolisa-ubuntu-experience-v1\n"
TEST_COMMAND = "python3 -m pytest -v --color=no -p no:cacheprovider test_checkout.py"
REPORT_COMMAND = "python3 ci_report.py"
SKILL_PROMPT = (
    "Read .qoder/skills/rpm-package-inspector/SKILL.md using a tool. "
    "Run its bash package file query once. Report the shell executable path, "
    "or the error and native alternative if it fails. Also show its tree "
    "installation command without running it. Do not install or edit anything. "
    "Answer in Chinese, within 100 words."
)
TOKEN_PROMPT = (
    f"Use Bash to execute exactly `{REPORT_COMMAND}` once to retrieve the last CI report. "
    "The shipping policy is free shipping for subtotal >= 100. From the report, "
    "identify the failing test, the boundary error and the smallest fix. "
    "Do not read files separately, rerun tests or edit files. "
    "Answer in Chinese, within 100 words."
)


def run(command, **kwargs):
    return subprocess.run(
        command, check=True, timeout=kwargs.pop("timeout", 30), **kwargs
    )


def text_run(command, **kwargs):
    return run(command, text=True, capture_output=True, **kwargs).stdout


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_owned(root):
    if root.is_symlink() or not (root / ".owned").is_file():
        raise RuntimeError(f"Not an owned demo directory: {root}; run prepare first")
    if (root / ".owned").read_text() != MARKER:
        raise RuntimeError(f"Invalid ownership marker: {root}")
    for name in ("runtime", "skills", "qoder-config"):
        if (root / name).is_symlink():
            raise RuntimeError(f"Refusing a symlink at {root / name}")


@contextlib.contextmanager
def locked(root):
    require_owned(root)
    with (root / ".lock").open("a") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError(
                "Another demo is running; finish it before resetting"
            ) from error
        yield


def mounts_under(root):
    # findmnt returns JSON-escaped target paths, including paths with spaces.
    info = json.loads(text_run(["findmnt", "--json", "--list", "--output", "TARGET"]))
    return [
        row["target"]
        for row in info["filesystems"]
        if Path(row["target"]) == root or root in Path(row["target"]).parents
    ]


def reset(root):
    mounts = mounts_under(root)
    if mounts:
        raise RuntimeError(f"Unmount before deleting demo data: {mounts}")
    runtime = root / "runtime"
    if runtime.exists():
        shutil.rmtree(runtime)
    for name in ("raw", "adapted", "baseline", "optimized"):
        workspace = runtime / name
        (workspace / ".qoder").mkdir(parents=True)
        if name in ("raw", "adapted"):
            target = root / ("skills" if name == "raw" else "runtime/mount/skills")
            (workspace / ".qoder/skills").symlink_to(target, target_is_directory=True)
        else:
            shutil.copy2(HERE / "fixtures/test_checkout.py", workspace)
    (runtime / "mount").mkdir()
    refresh_report(root)
    print("Round reset; Qoder credentials and original Skill retained.")


def refresh_report(root):
    result = subprocess.run(
        shlex.split(TEST_COMMAND),
        cwd=root / "runtime/optimized",
        capture_output=True,
        text=True,
        timeout=20,
        env=token_env(root, "fixture", False),
    )
    if result.returncode != 1 or "1 failed, 80 passed" not in result.stdout:
        raise RuntimeError(
            f"Unexpected test fixture result: {result.stdout}\n{result.stderr}"
        )
    for name in ("baseline", "optimized"):
        workspace = root / "runtime" / name
        (workspace / "ci-report.txt").write_text(result.stdout)
        shutil.copy2(HERE / "fixtures/ci_report.py", workspace)


def prepare(root):
    if root.exists():
        require_owned(root)
        print(f"Already prepared: {root}; use reset for a new participant.")
        return
    root.mkdir(parents=True, mode=0o700)
    (root / ".owned").write_text(MARKER)
    (root / "qoder-config").mkdir(mode=0o700)
    shutil.copytree(
        HERE / "fixtures/rpm-package-inspector", root / "skills/rpm-package-inspector"
    )
    source = root / "skills/rpm-package-inspector/SKILL.md"
    (root / "source.sha256").write_text(digest(source) + "\n")
    (root / "skillfs.toml").write_text(
        "[transforms.directive]\nenabled = false\n\n"
        '[transforms.os_adapter]\nenabled = true\ntarget_os = "auto"\n'
    )
    reset(root)


def source_check(root):
    source = root / "skills/rpm-package-inspector/SKILL.md"
    actual = digest(source)
    if actual != (root / "source.sha256").read_text().strip():
        raise RuntimeError("Original Skill changed; inspect it before continuing")
    print(f"Original SKILL.md unchanged: SHA-256 {actual}")


def token_env(root, name, enabled):
    env = os.environ.copy()
    # Prevent inherited database overrides from escaping this round's directory.
    for key in (
        "TOKENLESS_STATS_DB",
        "TOKENLESS_STASH_DB",
        "DSH_TOKENLESS_DATA_DIR",
        "DSH_TOKENLESS_STATS_DB",
        "DSH_TOKENLESS_STASH_DB",
    ):
        env.pop(key, None)
    env.update(
        TOKENLESS_DATA_DIR=str(root / "runtime" / f"stats-{name}"),
        TOKENLESS_COMPRESSION_ENABLED="1" if enabled else "0",
        TOKENLESS_STATS_ENABLED="1",
        TOKENLESS_SLS_ENABLED="0",
        PYTHONDONTWRITEBYTECODE="1",
        PYTEST_DISABLE_PLUGIN_AUTOLOAD="1",
    )
    return env


def stop_group(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def qoder_process(root, command, workspace, env, output, timeout, log):
    record_path = root / "runtime/qoder-process.json"
    record = {
        "command": command,
        "cwd": str(workspace),
        "ports": [],
        "log": str(log) if log else "interactive terminal",
        "lifetime_seconds": timeout,
    }
    record_path.write_text(json.dumps(record, indent=2))
    process = subprocess.Popen(
        command,
        cwd=workspace,
        env=env,
        stdout=output,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    record.update(pid=process.pid, stop=f"kill -TERM -- -{process.pid}")
    record_path.write_text(json.dumps(record, indent=2))
    try:
        return process.wait(timeout=timeout)
    finally:
        stop_group(process)
        record_path.unlink()


@contextlib.contextmanager
def mounted(root, skillfs):
    mount = root / "runtime/mount"
    if os.path.ismount(mount):
        raise RuntimeError(f"Mount already exists: {mount}")
    log = root / "runtime/skillfs.log"
    command = [
        skillfs,
        "mount",
        str(root / "skills"),
        str(mount),
        "--foreground",
        "--config",
        str(root / "skillfs.toml"),
    ]
    record = {
        "command": command,
        "cwd": str(root),
        "log": str(log),
        "ports": [],
        "lifetime": "until this command exits (maximum 120 seconds)",
        "stop": f"fusermount3 -u {shlex.quote(str(mount))}",
    }
    (root / "runtime/mount-process.json").write_text(json.dumps(record, indent=2))
    with log.open("w") as output:
        process = subprocess.Popen(
            command, cwd=root, stdout=output, stderr=output, start_new_session=True
        )
        record["pid"] = process.pid
        (root / "runtime/mount-process.json").write_text(json.dumps(record, indent=2))
        try:
            for _ in range(100):
                if os.path.ismount(mount):
                    break
                if process.poll() is not None:
                    raise RuntimeError(f"SkillFS exited; see {log}")
                time.sleep(0.1)
            else:
                raise RuntimeError(f"SkillFS readiness timed out; see {log}")
            yield mount
        finally:
            try:
                if os.path.ismount(mount):
                    run(["fusermount3", "-u", str(mount)], timeout=10)
            finally:
                stop_group(process)
            if os.path.ismount(mount):
                raise RuntimeError(f"Mount remains; run: {record['stop']}")
            (root / "runtime/mount-process.json").unlink()


def adapter_path(args):
    path = Path(args.adapter_dir).expanduser().resolve()
    for relative in (
        "qoder/.qoder-plugin/plugin.json",
        "common/hooks/compress_response_hook.py",
    ):
        if not (path / relative).is_file():
            raise RuntimeError(f"Missing installed adapter: {path / relative}")
    return path


def qoder(args, workspace, prompt, enabled, name):
    root = args.root
    stats = root / "runtime" / f"stats-{name}"
    if stats.exists():
        shutil.rmtree(stats)
    command = [
        args.qoder,
        "--config-dir",
        str(root / "qoder-config"),
        "--cwd",
        str(workspace),
        "--no-session-persistence",
        "--print",
        "--max-model-request-retries",
        "0",
        "--max-output-tokens",
        "700",
        "--tools",
        "Bash,Read",
        "--allowed-tools",
        "Bash,Read",
        "--strict-mcp-config",
        "--mcp-config",
        '{"mcpServers":{}}',
        "--plugin-dir",
        str(adapter_path(args) / "qoder"),
    ]
    if args.model:
        command += ["--model", args.model]
    if name in ("raw", "adapted"):
        visible_skills = root / ("skills" if name == "raw" else "runtime/mount/skills")
        command += ["--add-dir", str(visible_skills)]
    command += [prompt]
    log = root / "runtime" / f"{name}-answer.txt"
    print(f"Running {name}, deadline 90 seconds. Output: {log}", flush=True)
    with log.open("w") as output:
        status = qoder_process(
            root, command, workspace, token_env(root, name, enabled), output, 90, log
        )
    print(log.read_text())
    if status:
        raise RuntimeError(f"Qoder exited with status {status}; see {log}")
    if enabled:
        summary = json.loads(
            text_run(
                ["tokenless", "stats", "summary", "--json"],
                env=token_env(root, name, enabled),
            )
        )
        if summary["total"]["tokens_saved"] <= 0:
            raise RuntimeError(
                "No measured compression. Check Qoder hook loading and tool execution."
            )


def rehearsal(args):
    """Exercise the shipped Qoder hook without model calls or credentials."""
    root = args.root
    env = token_env(root, "rehearsal", True)
    result = run(
        shlex.split(REPORT_COMMAND),
        cwd=root / "runtime/optimized",
        capture_output=True,
        text=True,
        timeout=20,
        env=env,
    )
    event = {
        "session_id": "experience-rehearsal",
        "tool_use_id": "checkout-tests",
        "tool_name": "Bash",
        "tool_input": {"command": REPORT_COMMAND},
        "tool_response": {
            "stdout": result.stdout,
            "stderr": result.stderr,
            "exitCode": 0,
        },
    }
    env["TOKENLESS_AGENT_ID"] = "qoder-cli"
    hook = adapter_path(args) / "common/hooks/compress_response_hook.py"
    envelope = json.loads(
        text_run(["python3", str(hook)], input=json.dumps(event), env=env)
    )
    replacement = envelope.get("hookSpecificOutput", {}).get("updatedToolOutput")
    if not isinstance(replacement, str):
        raise RuntimeError(f"Qoder hook did not replace output: {envelope}")
    compressed = json.loads(replacement)["stdout"]
    for fact in (
        "test_free_shipping_at_threshold",
        "assert 5 == 0",
        "1 failed, 80 passed",
    ):
        if fact not in compressed:
            raise RuntimeError(f"Compression lost required diagnostic: {fact}")
    if len(compressed) >= len(result.stdout):
        raise RuntimeError("Fixture did not compress")
    (root / "runtime/before.txt").write_text(result.stdout)
    (root / "runtime/after.txt").write_text(compressed)
    env["TOKENLESS_COMPRESSION_ENABLED"] = "0"
    baseline = json.loads(
        text_run(["python3", str(hook)], input=json.dumps(event), env=env)
    )
    if baseline.get("hookSpecificOutput", {}).get("updatedToolOutput") is not None:
        raise RuntimeError("Baseline unexpectedly replaced output")
    print(
        f"Qoder hook rehearsal: {len(result.stdout)} -> {len(compressed)} characters "
        f"({1 - len(compressed) / len(result.stdout):.1%} smaller); failure preserved."
    )
    print("This is a local hook rehearsal, not a live Qoder/model session.")


def check(args):
    root = args.root
    for executable in (args.skillfs, args.qoder, "tokenless", "fusermount3", "findmnt"):
        if not shutil.which(executable):
            raise RuntimeError(f"Missing executable: {executable}")
    help_text = text_run([args.qoder, "--help"])
    for option in ("--config-dir", "--plugin-dir", "--no-session-persistence"):
        if option not in help_text:
            raise RuntimeError(f"Update Qoder: missing {option}")
    plugins = json.loads(
        text_run(
            [
                args.qoder,
                "--config-dir",
                str(root / "qoder-config"),
                "plugins",
                "list",
                "--json",
                "--plugin-dir",
                str(adapter_path(args) / "qoder"),
            ]
        )
    )
    if not any(
        plugin.get("name") == "tokenless"
        and plugin.get("enabled")
        and any(
            hook.get("event") == "PostToolUse"
            for hook in plugin.get("resources", {}).get("hooks", [])
        )
        for plugin in plugins
    ):
        raise RuntimeError("Qoder did not load the Tokenless PostToolUse hook")
    versions = {
        "qoder": text_run([args.qoder, "--version"]).strip(),
        "tokenless": text_run(["tokenless", "--version"]).strip(),
    }
    with mounted(root, args.skillfs) as mount:
        content = (mount / "skills/rpm-package-inspector/SKILL.md").read_text()
        if "dpkg -L bash" not in content or "apt-get install -y tree" not in content:
            raise RuntimeError(
                "Ubuntu OS adaptation failed; check SkillFS version/config"
            )
        print("Ubuntu view: rpm -ql bash -> dpkg -L bash; dnf -> apt-get")
    source_check(root)
    rehearsal(args)
    (root / "runtime/versions.json").write_text(json.dumps(versions, indent=2))
    print(
        "Local checks passed. Configure auth, then rehearse both live pairs before the event."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root", type=Path, default=Path.home() / ".local/share/anolisa-experience"
    )
    parser.add_argument("--skillfs", default="/usr/local/bin/skillfs")
    parser.add_argument("--qoder", default="qodercli")
    parser.add_argument("--model", default=os.environ.get("EXPERIENCE_MODEL"))
    parser.add_argument(
        "--adapter-dir",
        default=str(Path.home() / ".local/share/anolisa/adapters/tokenless"),
    )
    parser.add_argument(
        "action",
        choices=[
            "prepare",
            "check",
            "auth",
            "skillfs",
            "tokenless",
            "rehearsal",
            "stats",
            "reset",
            "cleanup",
        ],
    )
    parser.add_argument(
        "variant", nargs="?", choices=["raw", "adapted", "baseline", "optimized"]
    )
    args = parser.parse_args()
    if args.root.is_symlink():
        raise RuntimeError("Demo root must not be a symlink")
    args.root = args.root.expanduser().resolve()
    if args.action == "prepare":
        prepare(args.root)
        return
    with locked(args.root):
        if args.action == "check":
            check(args)
        elif args.action == "auth":
            print("Configure /model -> Custom with your Token Plan; exit when done.")
            status = qoder_process(
                args.root,
                [
                    args.qoder,
                    "--config-dir",
                    str(args.root / "qoder-config"),
                    "--cwd",
                    str(args.root / "runtime/raw"),
                ],
                args.root / "runtime/raw",
                os.environ.copy(),
                None,
                900,
                None,
            )
            if status:
                raise RuntimeError(f"Qoder auth exited with status {status}")
        elif args.action == "rehearsal":
            rehearsal(args)
        elif args.action == "skillfs":
            if args.variant not in ("raw", "adapted"):
                parser.error("skillfs requires raw or adapted")
            source_check(args.root)
            context = (
                mounted(args.root, args.skillfs)
                if args.variant == "adapted"
                else contextlib.nullcontext()
            )
            with context:
                path = (
                    args.root
                    / "runtime"
                    / args.variant
                    / ".qoder/skills/rpm-package-inspector/SKILL.md"
                )
                print(path.read_text(), flush=True)
                qoder(
                    args,
                    args.root / "runtime" / args.variant,
                    SKILL_PROMPT,
                    False,
                    args.variant,
                )
            source_check(args.root)
        elif args.action == "tokenless":
            if args.variant not in ("baseline", "optimized"):
                parser.error("tokenless requires baseline or optimized")
            qoder(
                args,
                args.root / "runtime" / args.variant,
                TOKEN_PROMPT,
                args.variant == "optimized",
                args.variant,
            )
        elif args.action == "stats":
            for name in ("baseline", "optimized"):
                print(f"\n{name} (tool-content estimates; not billing)", flush=True)
                run(
                    ["tokenless", "stats", "summary"],
                    env=token_env(args.root, name, name == "optimized"),
                )
                run(
                    ["tokenless", "stats", "list", "--limit", "3"],
                    env=token_env(args.root, name, name == "optimized"),
                )
        elif args.action == "reset":
            source_check(args.root)
            reset(args.root)
        elif args.action == "cleanup":
            if mounts_under(args.root):
                raise RuntimeError("Demo mounts remain; unmount before cleanup")
            shutil.rmtree(args.root)
            if args.root.exists():
                raise RuntimeError("Demo directory still exists")
            print(
                "Removed demo state, credentials and outputs; installed components retained."
            )


if __name__ == "__main__":

    def interrupted(signum, frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except (RuntimeError, OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("Interrupted; owned child processes stopped.", file=sys.stderr)
        sys.exit(130)
