#!/usr/bin/env python3
"""Run an interactive Qoder workspace with verified AW statistics in Herdr."""

import argparse
import fcntl
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import termios
import time
import tty
import uuid

import demo
from session_hooks import read, ticks, write
from session_observer import bridge

AW = Path(__file__).resolve().parents[1]
BINS = demo.DATA / "build/aw/debug"


def settings(root: Path, config: dict) -> dict:
    pid = os.getpid()
    start = ticks(pid)
    session = config["session_id"]
    runtime = {
        "runtime_id": str(uuid.uuid4()),
        "generation": 1,
        "binding_revision": 1,
        "environment_id": str(uuid.uuid4()),
        "process_ref": f"linux-pid-{pid}-start-{start}",
        "observation_source": "owned_child",
        "owner_id": "aw-session-launcher",
        "state": "running",
        "sequence": 1,
    }
    sec, tokenless = config["providers"]["sec-core"], config["providers"]["tokenless"]
    return {
        "runtime": runtime,
        "scope": {
            "environment_id": runtime["environment_id"],
            "execution_context_id": str(uuid.uuid4()),
            "actor_id": "qoder-main",
            "runtime_id": runtime["runtime_id"],
            "runtime_generation": 1,
            "binding_revision": 1,
            "session_id": session,
        },
        "agent_pid": pid,
        "agent_start_ticks": start,
        "journal": str(root / "journal"),
        "evidence": str(root / "evidence"),
        "provider": {
            "provider_id": "sec-core",
            "provider_version": sec["version"],
            "program": sec["program"],
            "args": [
                "-P",
                "-c",
                "from agent_sec_cli.aw_provider.runner import run_provider; import sys; run_provider(sys.stdin, sys.stdout)",
            ],
            "environment": {"PYTHONPATH": sec["source"], "PYTHONDONTWRITEBYTECODE": "1"},
        },
        "tokenless": {
            "provider_id": "tokenless",
            "provider_version": tokenless["version"],
            "program": tokenless["program"],
            "args": ["compress"],
            "allow_unrecoverable": True,
            "environment": {"TOKENLESS_DATA_DIR": str(root / "tokenless-data")},
        },
    }


def agent(root: Path) -> None:
    config = read(root / "launch.json")
    aw = settings(root, config)
    command = [
        config["qoder"],
        "--model",
        "auto",
        "--session-id",
        config["session_id"],
        "--settings",
        str(root / "qoder-settings.json"),
    ]
    config.update(
        aw=aw,
        agent_pid=os.getpid(),
        agent_start_ticks=aw["agent_start_ticks"],
        agent_pgid=os.getpgrp(),
        command=command,
        started_at_ms=int(time.time() * 1000),
        hook_bin=str(BINS / "aw-hook-cli"),
        stop_command=f"kill -TERM -- -{os.getpgrp()}",
    )
    write(root / "state.json", {"session_id": config["session_id"], "turn_id": None, "sequence": 0})
    write(root / "runtime.json", config)
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    for name, value in config["xdg"].items():
        if value is None:
            env.pop(name, None)
        else:
            env[name] = value
    os.chdir(config["workspace"])
    os.execve(config["qoder"], command, env)


def workspace_check(workspace: Path) -> None:
    if not workspace.is_dir():
        raise ValueError(f"workspace does not exist: {workspace}")
    for directory in (workspace, *workspace.parents):
        for name in ("settings.json", "settings.local.json"):
            path = directory / ".qoder" / name
            if path.is_file() and read(path).get("hooks"):
                raise ValueError(f"existing project hooks need coexistence review: {path}")


def live(pid: int, start: int) -> bool:
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        return fields[0] not in ("Z", "X") and int(fields[19]) == start
    except FileNotFoundError:
        return False


def stop_agent(root: Path) -> None:
    path = root / "runtime.json"
    if not path.exists():
        return
    config = read(path)
    pid, start = config["agent_pid"], config["agent_start_ticks"]
    try:
        if ticks(pid) != start:
            raise RuntimeError("refusing cleanup: registered Agent PID was reused")
    except FileNotFoundError:
        pass  # The group can outlive its leader.
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(config["agent_pgid"], sig)
        except ProcessLookupError:
            return
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            try:
                os.killpg(config["agent_pgid"], 0)
            except ProcessLookupError:
                return
            time.sleep(0.1)
    raise RuntimeError(f"Agent process group {config['agent_pgid']} survived cleanup")


def run(args: argparse.Namespace) -> None:
    if not args.allow_unrecoverable:
        raise ValueError("--allow-unrecoverable is required for native tool-output replacement")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ValueError("an interactive terminal is required")
    workspace = args.workspace.resolve()
    workspace_check(workspace)
    providers = demo.doctor(args.provider_dir)
    root = AW / "target/sessions" / str(uuid.uuid4())
    root.mkdir(parents=True, mode=0o700)
    for name in (
        "calls",
        "turns",
        "errors",
        "journal",
        "evidence",
        "adoption-journal",
        "tokenless-data",
    ):
        (root / name).mkdir(mode=0o700)
    command_hook = shlex.join([sys.executable, str(AW / "scripts/session_hooks.py"), str(root)])
    hooks = {}
    for event in (
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PostToolUseFailure",
    ):
        entry = {"hooks": [{"type": "command", "command": command_hook, "timeout": 30}]}
        if event in ("PreToolUse", "PostToolUse", "PostToolUseFailure"):
            entry["matcher"] = "Bash"
        hooks[event] = [entry]
    write(root / "qoder-settings.json", {"hooks": hooks, "general": {"enableAutoUpdate": False}})
    launch = {
        "workspace": str(workspace),
        "providers": providers,
        "qoder": shutil.which("qodercli"),
        "session_id": str(uuid.uuid4()),
        "duration_seconds": args.duration,
        "xdg": {key: os.environ.get(key) for key in ("XDG_CONFIG_HOME", "XDG_STATE_HOME")},
    }
    write(root / "launch.json", launch)
    print(f"Workspace: {workspace}\nSession evidence (includes Bash outputs): {root}", flush=True)
    print("Type tasks directly in Qoder. Ctrl+B, then Q ends this session.", flush=True)
    binary = str(demo.DATA / "herdr/herdr")
    server = client = observer = None
    master = slave = None
    original_terminal = termios.tcgetattr(sys.stdin.fileno())
    ownership = {
        "launcher_pid": os.getpid(),
        "launcher_command": [sys.executable, *sys.argv],
        "cwd": str(workspace),
        "duration_seconds": args.duration,
        "stop_command": f"kill -TERM {os.getpid()}",
        "server_log": str(root / "herdr.log"),
    }
    with tempfile.TemporaryDirectory(prefix="aw-session-") as temporary:
        runtime = Path(temporary)
        endpoint = runtime / "api.sock"
        shutil.copyfile(AW / "integrations/herdr/config.toml", runtime / "config.toml")
        env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
        env.update(
            HERDR_SOCKET_PATH=str(endpoint),
            HERDR_CONFIG_PATH=str(runtime / "config.toml"),
            XDG_CONFIG_HOME=str(runtime / "config"),
            XDG_STATE_HOME=str(runtime / "state"),
            TERM="xterm-256color",
            PYTHONDONTWRITEBYTECODE="1",
        )
        ownership.update(
            socket=str(endpoint),
            temporary_directory=str(runtime),
            server_command=[binary, "server"],
            client_command=[binary],
        )
        write(root / "ownership.json", ownership)
        with (root / "herdr.log").open("wb") as log:
            try:
                server = subprocess.Popen(
                    [binary, "server"],
                    cwd=workspace,
                    env=env,
                    stdout=log,
                    stderr=log,
                    start_new_session=True,
                )
                ownership.update(server_pid=server.pid, server_stop=f"kill -TERM -- -{server.pid}")
                write(root / "ownership.json", ownership)
                deadline = time.monotonic() + 10
                while not endpoint.exists() and time.monotonic() < deadline:
                    if server.poll() is not None:
                        raise RuntimeError("Herdr server exited; inspect herdr.log")
                    time.sleep(0.1)
                bridge.rpc(endpoint, "ping", {})
                bridge.rpc(
                    endpoint,
                    "workspace.create",
                    {"label": workspace.name, "cwd": str(workspace), "focus": True},
                )
                pane = bridge.rpc(endpoint, "pane.list", {})["panes"][0]["pane_id"]
                ownership["pane_id"] = pane
                master, slave = pty.openpty()
                size = fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b"\0" * 8)
                fcntl.ioctl(slave, termios.TIOCSWINSZ, size)
                client = subprocess.Popen(
                    [binary],
                    cwd=workspace,
                    env=env,
                    stdin=slave,
                    stdout=slave,
                    stderr=slave,
                    start_new_session=True,
                )
                ownership.update(client_pid=client.pid, client_stop=f"kill -TERM -- -{client.pid}")
                write(root / "ownership.json", ownership)
                command = [sys.executable, str(Path(__file__).resolve()), "--agent", str(root)]
                bridge.rpc(
                    endpoint,
                    "pane.send_input",
                    {"pane_id": pane, "text": shlex.join(command) + "\n"},
                )
                tty.setraw(sys.stdin.fileno())
                deadline = time.monotonic() + args.duration
                while time.monotonic() < deadline:
                    if client.poll() is not None:
                        break
                    if server.poll() is not None:
                        raise RuntimeError("Herdr server exited unexpectedly")
                    ready, _, _ = select.select([master, sys.stdin.fileno()], [], [], 0.1)
                    if master in ready:
                        chunk = os.read(master, 65536)
                        sys.stdout.buffer.write(chunk)
                        sys.stdout.buffer.flush()
                    if sys.stdin.fileno() in ready:
                        chunk = os.read(sys.stdin.fileno(), 65536)
                        if not chunk:
                            break
                        os.write(master, chunk)
                    current_size = fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b"\0" * 8)
                    if current_size != size:
                        size = current_size
                        fcntl.ioctl(master, termios.TIOCSWINSZ, size)
                    if (root / "runtime.json").exists():
                        config = read(root / "runtime.json")
                        if not live(config["agent_pid"], config["agent_start_ticks"]):
                            break
                        if observer is None:
                            observer_command = [
                                sys.executable,
                                str(AW / "scripts/session_observer.py"),
                                str(root),
                                str(endpoint),
                                pane,
                                str(BINS),
                                str(args.duration),
                            ]
                            observer = subprocess.Popen(
                                observer_command,
                                stdout=log,
                                stderr=log,
                                env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
                                start_new_session=True,
                            )
                            ownership.update(
                                observer_pid=observer.pid,
                                observer_command=observer_command,
                                observer_stop=f"kill -TERM -- -{observer.pid}",
                            )
                            write(root / "ownership.json", ownership)
                        elif observer.poll() is not None:
                            raise RuntimeError("AW observer exited; inspect herdr.log")
                else:
                    ownership["end_reason"] = "session duration reached"
            finally:
                signal.signal(signal.SIGINT, signal.SIG_IGN)
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, original_terminal)
                try:
                    stop_agent(root)
                finally:
                    for process in (observer, client, server):
                        if process is not None and process.poll() is None:
                            os.killpg(process.pid, signal.SIGTERM)
                            try:
                                process.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                os.killpg(process.pid, signal.SIGKILL)
                                process.wait(timeout=5)
                    for fd in (master, slave):
                        if fd is not None:
                            os.close(fd)
                    sys.stdout.write(
                        "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[?2031l\x1b[<u\x1b[?7h\x1b[?1049l\x1b[?25h\x1b[0m"
                    )
                    sys.stdout.flush()
                    if (root / "runtime.json").exists():
                        config = read(root / "runtime.json")
                        if live(config["agent_pid"], config["agent_start_ticks"]):
                            raise RuntimeError(
                                "owned Qoder process survived cleanup; inspect runtime.json"
                            )
                    ownership["cleanup"] = (
                        "owned processes stopped; native Qoder history and workspace retained"
                    )
                    write(root / "ownership.json", ownership)
    print(f"Session ended. Workspace and Qoder history retained.\nAW evidence: {root}")


def main() -> int:
    os.umask(0o077)
    if len(sys.argv) == 3 and sys.argv[1] == "--agent":
        agent(Path(sys.argv[2]))
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--provider-dir", type=Path, default=AW / "providers")
    parser.add_argument("--allow-unrecoverable", action="store_true")
    parser.add_argument(
        "--duration",
        type=int,
        default=3600,
        help="session deadline in seconds, 60..14400 (default one hour)",
    )
    args = parser.parse_args()
    if not 60 <= args.duration <= 14400:
        parser.error("--duration must be 60..14400 seconds")
    signal.signal(signal.SIGTERM, lambda _signal, _frame: sys.exit(130))
    try:
        run(args)
    except (
        OSError,
        ValueError,
        RuntimeError,
        subprocess.SubprocessError,
        KeyboardInterrupt,
    ) as error:
        print(f"AW session ended with an error: {error or 'interrupted'}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
