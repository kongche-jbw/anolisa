#!/usr/bin/env python3
"""Attach a native Herdr terminal for a Qoder or Codex invocation."""

import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import termios
import time
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
    aw = None
    command = [config["agent_program"]]
    if config["agent_kind"] == "qoder":
        aw = settings(root, config)
        command += [
            "--model",
            "auto",
            "--session-id",
            config["session_id"],
            "--settings",
            str(root / "qoder-settings.json"),
        ]
    command += config.get("agent_args", [])
    config.update(
        aw=aw,
        agent_pid=os.getpid(),
        agent_start_ticks=ticks(os.getpid()),
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
    # Do not recursively attach Herdr for commands run inside an agent.
    for name in (
        "BASH_FUNC_qoder%%",
        "BASH_FUNC_codex%%",
        "_AW_CHECKOUT",
        "_AW_PYTHON",
        "_AW_ALLOW_UNRECOVERABLE",
    ):
        env.pop(name, None)
    os.chdir(config["workspace"])
    os.execve(command[0], command, env)


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
    kind = getattr(args, "agent_kind", "qoder")
    agent_args = getattr(args, "agent_args", [])
    if agent_args[:1] == ["--"]:
        agent_args = agent_args[1:]
    if kind == "qoder" and any(
        arg.split("=", 1)[0] in ("--session-id", "--settings", "--resume", "--continue", "-c", "-r")
        for arg in agent_args
    ):
        raise ValueError("Qoder session identity and hook settings are owned by the launcher")
    if kind == "qoder" and not args.allow_unrecoverable:
        raise ValueError("--allow-unrecoverable is required for native tool-output replacement")
    if not sys.stdin.isatty() or not sys.stdout.isatty():
        raise ValueError("an interactive terminal is required")
    workspace = args.workspace.resolve()
    if kind == "qoder":
        workspace_check(workspace)
    providers = demo.doctor(args.provider_dir, agent=kind == "qoder")
    program = shutil.which("qodercli" if kind == "qoder" else "codex")
    if not program:
        raise ValueError(f"{kind} executable is not installed on PATH")
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
    if kind == "qoder":
        write(
            root / "qoder-settings.json", {"hooks": hooks, "general": {"enableAutoUpdate": False}}
        )
    launch = {
        "workspace": str(workspace),
        "providers": providers,
        "agent_kind": kind,
        "agent_program": program,
        "agent_args": agent_args,
        "session_id": str(uuid.uuid4()),
        "duration_seconds": args.duration,
        "xdg": {key: os.environ.get(key) for key in ("XDG_CONFIG_HOME", "XDG_STATE_HOME")},
    }
    write(root / "launch.json", launch)
    print(f"Workspace: {workspace}\nSession evidence (includes Bash outputs): {root}", flush=True)
    print(f"Type tasks directly in {kind}. Ctrl+B, then q returns to cosh.", flush=True)
    if kind == "codex":
        write(
            root / "provider-details.json",
            {"status": "not_connected", "session_id": launch["session_id"]},
        )
    binary = str(demo.DATA / "herdr/herdr")
    server = client = observer = None
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
        panel_command = shlex.join(
            [sys.executable, str(AW / "scripts/provider_details.py"), str(root)]
        )
        config_text = (AW / "integrations/herdr/config.toml").read_text()
        config_text += (
            '\n[[keys.command]]\nkey = "prefix+p"\ntype = "popup"\n'
            'width = "90%"\nheight = "85%"\ndescription = "AW Provider details"\n'
            f"command = {json.dumps(panel_command)}\n"
        )
        (runtime / "config.toml").write_text(config_text)
        env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
        for name in ("BASH_FUNC_qoder%%", "BASH_FUNC_codex%%"):
            env.pop(name, None)
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
                client = subprocess.Popen(
                    [binary],
                    cwd=workspace,
                    env=env,
                    stdin=sys.stdin,
                    stdout=sys.stdout,
                    stderr=sys.stderr,
                )
                ownership.update(client_pid=client.pid, client_stop=f"kill -TERM {client.pid}")
                write(root / "ownership.json", ownership)
                command = [sys.executable, str(Path(__file__).resolve()), "--agent", str(root)]
                bridge.rpc(
                    endpoint,
                    "pane.send_input",
                    {"pane_id": pane, "text": shlex.join(command) + "\n"},
                )
                deadline = time.monotonic() + args.duration
                codex_metadata_at = 0.0
                codex_sequence = 0
                while time.monotonic() < deadline:
                    if client.poll() is not None:
                        break
                    if server.poll() is not None:
                        raise RuntimeError("Herdr server exited unexpectedly")
                    time.sleep(0.1)
                    if (root / "runtime.json").exists():
                        config = read(root / "runtime.json")
                        if not live(config["agent_pid"], config["agent_start_ticks"]):
                            break
                        if kind == "codex":
                            if time.monotonic() >= codex_metadata_at:
                                codex_sequence += 1
                                bridge.publish(
                                    endpoint,
                                    pane,
                                    {
                                        "aw": "AW hooks: not connected",
                                        "aw_sec": "SecCore: not invoked",
                                        "aw_tokenless": "Tokenless: not invoked",
                                        "aw_usage": "Ctrl+B p: Provider configuration",
                                    },
                                    codex_sequence,
                                )
                                codex_metadata_at = time.monotonic() + 2
                            continue
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
                signal.signal(signal.SIGHUP, signal.SIG_IGN)
                termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, original_terminal)
                try:
                    stop_agent(root)
                finally:
                    for process in (observer, client, server):
                        if process is not None and process.poll() is None:
                            if process is client:
                                process.terminate()
                            else:
                                os.killpg(process.pid, signal.SIGTERM)
                            try:
                                process.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                if process is client:
                                    process.kill()
                                else:
                                    os.killpg(process.pid, signal.SIGKILL)
                                process.wait(timeout=5)
                    sys.stdout.write(
                        "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[?2031l\x1b[<u\x1b[?7h\x1b[?1049l\x1b[?25h\x1b[0m"
                    )
                    sys.stdout.flush()
                    if (root / "runtime.json").exists():
                        config = read(root / "runtime.json")
                        if live(config["agent_pid"], config["agent_start_ticks"]):
                            raise RuntimeError(
                                "owned agent process survived cleanup; inspect runtime.json"
                            )
                    ownership["cleanup"] = (
                        "owned processes stopped; native agent history and workspace retained"
                    )
                    write(root / "ownership.json", ownership)
    print(f"Session ended. Workspace and native agent history retained.\nAW evidence: {root}")


def main() -> int:
    os.umask(0o077)
    if len(sys.argv) == 3 and sys.argv[1] == "--agent":
        agent(Path(sys.argv[2]))
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--agent-kind", choices=("qoder", "codex"), default="qoder")
    parser.add_argument("agent_args", nargs=argparse.REMAINDER)
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
    signal.signal(signal.SIGHUP, lambda _signal, _frame: sys.exit(130))
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
