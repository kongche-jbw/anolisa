#!/usr/bin/env python3
"""Run one bounded, isolated QwenPaw or Hermes model case through AW.

The runtime directory must contain the previously installed source/ and venv/.
This fixture uses qwen3.7-plus through the CN Token Plan endpoint. Credentials
remain in memory or the supplied private file; output retains logs and evidence.
QwenPaw exercises its public Agent runtime; Hermes exercises its native CLI.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path
from types import FrameType
from typing import Any, BinaryIO

REPO = Path(__file__).resolve().parents[4]
ENDPOINT = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
MODEL = "qwen3.7-plus"


def arguments(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--framework", choices=("qwenpaw", "hermes"), required=True)
    parser.add_argument("--mode", choices=("allow", "deny", "ask"), required=True)
    parser.add_argument(
        "--runtime-dir", type=Path, required=True, help="Contains source/ and venv/"
    )
    parser.add_argument("--output", type=Path, required=True, help="New owned case directory")
    parser.add_argument("--aw", type=Path, required=True, help="Built AW executable")
    parser.add_argument(
        "--key-file", type=Path, required=True, help="Current-user private credential"
    )
    args = parser.parse_args(argv)
    if os.path.lexists(args.output.expanduser()):
        parser.error("--output must be a new directory")
    for name in ("runtime_dir", "output", "aw", "key_file"):
        setattr(args, name, getattr(args, name).expanduser().resolve())
    if sys.platform != "linux":
        parser.error("the AW native lab requires Linux")
    if len(os.fsencode(args.output / "aw.sock")) >= 108:
        parser.error("--output is too long for a Unix socket; choose a shorter path")
    if not (args.runtime_dir / "source").is_dir():
        parser.error("--runtime-dir must contain source/")
    python = args.runtime_dir / "venv/bin/python"
    if not python.is_file() or not os.access(python, os.X_OK):
        parser.error("--runtime-dir must contain executable venv/bin/python")
    if not args.aw.is_file() or not os.access(args.aw, os.X_OK):
        parser.error("--aw must be an executable file")
    try:
        metadata = args.key_file.stat()
    except OSError:
        parser.error("--key-file must be an accessible private file")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.getuid()
        or stat.S_IMODE(metadata.st_mode) & 0o077
    ):
        parser.error("--key-file must be owned by the current user without group/other access")
    return args


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


class Processes:
    """Keep a ledger local to this case, avoiding concurrent case write races."""

    def __init__(self, output: Path) -> None:
        self.output = output
        self.records: list[dict[str, Any]] = []
        self.owned: list[tuple[subprocess.Popen[bytes], dict[str, Any], BinaryIO]] = []

    def save(self) -> None:
        write_json(self.output / "processes.json", self.records)

    def start(
        self, command: list[str], cwd: Path, name: str, seconds: int, environment: dict[str, str]
    ) -> subprocess.Popen[bytes]:
        logfile = self.output / f"{name}.log"
        record: dict[str, Any] = {
            "command": command,
            "cwd": str(cwd),
            "pid": None,
            "process_group": None,
            "ports": [],
            "socket": str(self.output / "aw.sock"),
            "log": str(logfile),
            "deadline_seconds": seconds,
            "started_at": time.time(),
            "stop_command": None,
        }
        self.records.append(record)
        self.save()
        log = logfile.open("wb")
        try:
            process = subprocess.Popen(
                command,
                cwd=cwd,
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        except BaseException:
            log.close()
            raise
        record.update(
            pid=process.pid,
            process_group=process.pid,
            stop_command=f"kill -TERM -- -{process.pid}",
        )
        self.owned.append((process, record, log))
        self.save()
        return process

    def close(self) -> bool:
        clean = True
        for process, record, log in reversed(self.owned):
            stopped = False
            try:
                # WNOWAIT keeps the leader's PID reserved through the final
                # group signal, including when it exited before its children.
                observe_exit(process)
                for sig in (signal.SIGTERM, signal.SIGKILL):
                    try:
                        os.killpg(process.pid, sig)
                    except ProcessLookupError:
                        pass
                    deadline = time.monotonic() + 5
                    while time.monotonic() < deadline:
                        if observe_exit(process) is not None and group_stopped(
                            process.pid, deadline
                        ):
                            process.wait(timeout=1)
                            stopped = True
                            break
                        time.sleep(0.05)
                    if stopped:
                        break
            except (OSError, TimeoutError, subprocess.TimeoutExpired) as error:
                # A reaped leader cannot prove ownership of a reused group ID.
                # Report the failure and continue cleaning other owned groups.
                record["cleanup_error"] = type(error).__name__
            clean = clean and stopped
            record.update(
                exit_code=process.returncode,
                finished=process.returncode is not None,
                ended_at=time.time(),
                process_group_stopped=stopped,
            )
            log.close()
            self.save()
        return clean


def observe_exit(process: subprocess.Popen[bytes]) -> int | None:
    if process.returncode is not None:
        raise ChildProcessError("owned leader was reaped before group cleanup")
    info = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    if info is None:
        return None
    return info.si_status if info.si_code == os.CLD_EXITED else -info.si_status


def wait_unreaped(process: subprocess.Popen[bytes], seconds: float) -> int:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        code = observe_exit(process)
        if code is not None:
            return code
        time.sleep(0.05)
    raise subprocess.TimeoutExpired(process.args, seconds)


def group_stopped(pgid: int, deadline: float) -> bool:
    scanned = 0

    def state(path: Path) -> bytes | None:
        nonlocal scanned
        scanned += 1
        if scanned > 65536 or time.monotonic() >= deadline:
            raise TimeoutError("process-group scan exceeded its bound")
        try:
            fields = path.read_bytes().rsplit(b")", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError):
            return None
        return fields[0] if int(fields[2]) == pgid else None

    for process in Path("/proc").iterdir():
        if not process.name.isdecimal():
            continue
        status = state(process / "stat")
        if status is None:
            continue
        if status not in (b"Z", b"X"):
            return False
        # A zombie leader can still have live threads; inspect matching tasks.
        try:
            for task in (process / "task").iterdir():
                if state(task / "stat") not in (None, b"Z", b"X"):
                    return False
        except (FileNotFoundError, ProcessLookupError):
            continue
    return True


def configuration(args: argparse.Namespace) -> dict[str, Any]:
    python = args.runtime_dir / "venv/bin/python"
    fixture = REPO / f"src/aw/tests/native/{args.framework}/hook_command.py"
    providers: dict[str, Any] = {}
    events: dict[str, Any] = {}
    for event, labels in (
        ("tool.before", ("before-a", "before-b")),
        ("tool.after", ("after-a", "after-b")),
    ):
        steps = []
        for label in labels:
            behavior = args.mode if label == "before-a" else "allow"
            providers[label] = {
                "protocol": "native-hook/v1alpha1",
                "transport": {
                    "type": "stdio",
                    "argv": [
                        str(python),
                        str(fixture),
                        label,
                        behavior,
                        str(args.output / "trace.jsonl"),
                    ],
                    "location": "agent",
                },
                "timeout_ms": 5000,
                "max_output_bytes": 65536,
                "config": {},
            }
            steps.append({"id": label, "provider": label, "native": {}})
        events[event] = {"enabled": True, "required": True, "steps": steps}
    if args.framework == "qwenpaw":
        argv = [str(python), str(REPO / "src/aw/tests/native/qwenpaw/live_agent.py")]
    else:
        argv = [
            str(python),
            "-m",
            "hermes_cli.main",
            "chat",
            "--oneshot",
            "--max-turns",
            "3",
            "--run-budget",
            "150",
            "--provider",
            "custom",
            "--model",
            MODEL,
            "--accept-hooks",
            "--ignore-rules",
            "--toolsets",
            "terminal",
            "--quiet",
            "--query-file",
            str(args.output / "prompt.txt"),
        ]
    return {
        "apiVersion": "aw/v1alpha1",
        "kind": "AWConfiguration",
        "metadata": {"name": f"native-{args.framework}-{args.mode}"},
        "spec": {
            "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
            "execution": {"guarantee": "native_hook", "default_event_budget_ms": 15000},
            "audit": {"enabled": True, "payload": "metadata_only"},
            "agents": {args.framework: {"adapter": args.framework, "argv": argv}},
            "providers": providers,
            "events": events,
        },
    }


def result(args: argparse.Namespace, code: int) -> dict[str, Any]:
    trace = args.output / "trace.jsonl"
    rows = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
    labels = [row["label"] for row in rows]
    marker = args.output / "work/answer.txt"
    marker_value = marker.read_text() if marker.exists() else None
    expected_labels = ["before-a", "before-b", "after-a", "after-b"]
    expected_code = 0
    if args.framework == "qwenpaw":
        expected_labels = {
            "allow": ["before-a", "before-b", "after-b", "after-a"],
            "deny": ["before-a", "after-b", "after-a"],
            "ask": ["before-a"],
        }[args.mode]
        expected_code = 1 if args.mode == "ask" else 0
    passed = (
        code == expected_code
        and labels == expected_labels
        and marker.exists() == (args.mode == "allow")
        and (args.mode != "allow" or marker_value == "42\n")
    )
    if args.framework == "qwenpaw" and args.mode == "ask":
        passed = passed and "does not support ask/approval" in (
            args.output / "agent.log"
        ).read_text(errors="replace")
    return {
        "framework": args.framework,
        "mode": args.mode,
        "exit_code": code,
        "labels": labels,
        "marker_exists": marker.exists(),
        "marker_value": marker_value,
        "passed": passed,
    }


def interrupted(signum: int, _frame: FrameType | None) -> None:
    raise SystemExit(128 + signum)


def prepare(args: argparse.Namespace) -> tuple[list[str], dict[str, str]]:
    work = args.output / "work"
    work.mkdir()
    socket = args.output / "aw.sock"
    config_path = args.output / "aw.json"
    home = args.output / "home"
    state = args.output / "state"
    write_json(config_path, configuration(args))
    (args.output / "prompt.txt").write_text(
        "Use terminal exactly once to run: printf '42\\n' > answer.txt. "
        "This is a coding verification fixture in the current temporary workspace. "
        "Then report completion. If the tool is denied or requires approval, "
        "report that and stop without retrying or using another tool.\n"
    )
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("AW_", "QWENPAW_", "HERMES_"))
    }
    environment.update(PYTHONDONTWRITEBYTECODE="1", AW_TEST_MODEL_KEY_FILE=str(args.key_file))
    command = [
        str(args.aw),
        "run",
        args.framework,
        "--config",
        str(config_path),
        "--state-dir",
        str(state),
        "--socket",
        str(socket),
    ]
    if args.framework == "qwenpaw":
        home.mkdir()
        environment["QWENPAW_WORKING_DIR"] = str(home)
        environment["QWENPAW_SECRET_DIR"] = str(home / "secret")
    else:
        native = args.output / "native.json"
        write_json(
            native,
            {
                "model": {
                    "provider": "custom",
                    "default": MODEL,
                    "base_url": ENDPOINT,
                    "key_env": "AW_TEST_API_KEY",
                    "api_mode": "chat_completions",
                },
                "terminal": {"backend": "local", "cwd": str(work)},
                "agent": {"max_turns": 3},
                "memory": {"memory_enabled": False, "user_profile_enabled": False},
                "database": {"journal_mode": "delete"},
            },
        )
        environment["AW_TEST_API_KEY"] = args.key_file.read_text().strip()
        environment["PYTHONPATH"] = str(args.runtime_dir / "source")
        command.extend(["--native-config", str(native)])
    return command, environment


def run(args: argparse.Namespace) -> int:
    args.output.mkdir(parents=True, exist_ok=False, mode=0o700)
    processes = Processes(args.output)
    work = args.output / "work"
    socket = args.output / "aw.sock"
    config_path = args.output / "aw.json"
    home = args.output / "home"
    state = args.output / "state"
    try:
        command, environment = prepare(args)
        # Model credentials are added only to the Agent's environment.
        daemon = processes.start(
            [
                str(args.aw),
                "serve",
                "--config",
                str(config_path),
                "--socket",
                str(socket),
                "--idle-timeout",
                "30",
            ],
            work,
            "daemon",
            210,
            dict(os.environ),
        )
        deadline = time.monotonic() + 5
        while not socket.exists():
            if observe_exit(daemon) is not None:
                raise RuntimeError("daemon exited before readiness")
            if time.monotonic() >= deadline:
                raise TimeoutError("daemon did not create its socket within 5 seconds")
            time.sleep(0.05)
        child = processes.start(command, work, "agent", 180, environment)
        try:
            code = wait_unreaped(child, 180)
        except subprocess.TimeoutExpired as error:
            raise TimeoutError("native Agent exceeded 180 seconds") from error
        evidence = result(args, code)
        write_json(args.output / "result.json", evidence)
        print(json.dumps(evidence), flush=True)
        return 0 if evidence["passed"] else 1
    finally:
        clean = processes.close()
        # These directories were absent before this case and contain only its
        # generated bindings, QwenPaw plugins/profiles, or isolated Hermes home.
        if clean:
            for directory in (state, home):
                if directory.exists():
                    shutil.rmtree(directory)
            socket.unlink(missing_ok=True)
        cleanup = {
            "process_groups_stopped": clean,
            "socket_absent": not socket.exists(),
            "generated_directories_absent": not state.exists() and not home.exists(),
        }
        write_json(args.output / "cleanup.json", cleanup)
        if not all(cleanup.values()):
            raise RuntimeError("owned cleanup incomplete; inspect processes.json and cleanup.json")


def main() -> int:
    args = arguments()
    previous_umask = os.umask(0o077)
    handlers = {sig: signal.signal(sig, interrupted) for sig in (signal.SIGINT, signal.SIGTERM)}
    try:
        return run(args)
    finally:
        for sig, handler in handlers.items():
            signal.signal(sig, handler)
        os.umask(previous_umask)


if __name__ == "__main__":
    raise SystemExit(main())
