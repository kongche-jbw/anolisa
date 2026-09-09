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
import codex_hooks
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
            "actor_id": config["agent_kind"] + "-main",
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
    command = [config["agent_program"]]
    if config["agent_kind"] == "qoder":
        command += [
            "--model",
            "auto",
            "--session-id",
            config["session_id"],
            "--settings",
            str(root / "qoder-settings.json"),
        ]
    else:
        aw.pop("tokenless")
        aw["scope"].pop("session_id")
    agent_args = config.get("agent_args", [])
    if config["agent_kind"] == "codex":
        # Codex exec has its own config list; root-level overrides can be lost
        # when exec receives -c. Attach hooks at the final command's option layer.
        boundary = agent_args.index("--") if "--" in agent_args else len(agent_args)
        agent_args = agent_args[:boundary] + codex_hooks.arguments() + agent_args[boundary:]
    command += agent_args
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
    write(
        root / "state.json",
        {
            "session_id": config["session_id"] if config["agent_kind"] == "qoder" else None,
            "turn_id": None,
            "sequence": 0,
        },
    )
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
        "BASH_FUNC_qodercli%%",
        "BASH_FUNC_codex%%",
        "_AW_CHECKOUT",
        "_AW_PYTHON",
        "_AW_ALLOW_UNRECOVERABLE",
    ):
        env.pop(name, None)
    if config["agent_kind"] == "codex":
        env["AW_CODEX_SESSION_DIR"] = str(root)
    else:
        env.pop("AW_CODEX_SESSION_DIR", None)
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


def prepare(args: argparse.Namespace) -> Path:
    kind = args.agent_kind
    agent_args = args.agent_args
    if agent_args[:1] == ["--"]:
        agent_args = agent_args[1:]
    if kind == "qoder" and any(
        arg.split("=", 1)[0] in ("--session-id", "--settings", "--resume", "--continue", "-c", "-r")
        for arg in agent_args
    ):
        raise ValueError("Qoder session identity and hook settings are owned by the launcher")
    if kind == "codex" and any(
        arg.split("=", 1)[0] in ("--remote", "-C", "--cd") for arg in agent_args
    ):
        raise ValueError("Codex attachment requires the local process and current workspace")
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
        "provider_dir": str(args.provider_dir.resolve()),
        "allow_unrecoverable": args.allow_unrecoverable,
        "deadline_monotonic": time.monotonic() + args.duration,
        "xdg": {key: os.environ.get(key) for key in ("XDG_CONFIG_HOME", "XDG_STATE_HOME")},
    }
    write(root / "launch.json", launch)
    print(f"Workspace: {workspace}\nSession evidence (includes Bash outputs): {root}", flush=True)
    print(f"Type tasks directly in {kind}. Ctrl+B, then q returns to cosh.", flush=True)
    if kind == "codex":
        print(
            "Codex AW: waiting for native hook. First use: /hooks -> trust AW hooks; then run a shell task.\nSecCore: post-tool inspection only. Tokenless: output replacement unsupported in this adapter.",
            flush=True,
        )
        write(
            root / "provider-details.json",
            {"status": "waiting for Codex hook; review /hooks", "session_id": None},
        )
    return root


def attach(args: argparse.Namespace) -> None:
    from session_panels import register

    group = args.attach.resolve()
    owner = read(group / "ownership.json")
    config = read(group / "launch.json")
    if not live(owner["launcher_pid"], owner["launcher_start_ticks"]):
        raise ValueError("the owning Herdr launcher is no longer running")
    if os.environ.get("HERDR_SOCKET_PATH") != owner["socket"]:
        raise ValueError("attachment must run inside the owning Herdr instance")
    pane = os.environ.get("HERDR_PANE_ID")
    if not pane:
        raise ValueError("native Herdr pane identity is missing")
    process = bridge.rpc(Path(owner["socket"]), "pane.process_info", {"pane_id": pane})
    parent = os.getppid()
    for _ in range(64):
        if parent == process["shell_pid"]:
            break
        if parent <= 1:
            raise ValueError("attachment is not descended from the selected pane shell")
        parent = int(Path(f"/proc/{parent}/stat").read_text().rsplit(")", 1)[1].split()[1])
    else:
        raise ValueError("pane ancestry exceeds limit")
    args.duration = int(config["deadline_monotonic"] - time.monotonic())
    if args.duration <= 0:
        raise ValueError("Herdr instance deadline reached")
    args.provider_dir = Path(config["provider_dir"])
    args.allow_unrecoverable = config["allow_unrecoverable"]
    for key, value in config["xdg"].items():
        if value is None:
            os.environ.pop(key, None)
        else:
            os.environ[key] = value
    root = prepare(args)
    write(
        root / "ownership.json",
        {
            "launcher_pid": os.getpid(),
            "launcher_start_ticks": ticks(os.getpid()),
            "launcher_command": [sys.executable, *sys.argv],
            "cwd": str(args.workspace.resolve()),
            "duration_seconds": args.duration,
            "group_root": str(group),
            "pane_id": pane,
            "socket": owner["socket"],
            "stop_command": f"kill -TERM {os.getpid()}",
            "server_log": owner["server_log"],
        },
    )
    register(group, root, pane)
    agent(root)


def run(args: argparse.Namespace) -> None:
    from session_panels import Panels, pane_shell, register, registrations

    root = prepare(args)
    workspace = args.workspace.resolve()
    (root / "panels").mkdir(mode=0o700)
    binary = str(demo.DATA / "herdr/herdr")
    server = client = panels = None
    original_terminal = termios.tcgetattr(sys.stdin.fileno())
    ownership = {
        "launcher_pid": os.getpid(),
        "launcher_start_ticks": ticks(os.getpid()),
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
            [sys.executable, str(AW / "scripts/provider_details.py"), "--group", str(root)]
        )
        config_text = (AW / "integrations/herdr/config.toml").read_text()
        config_text = config_text.replace(
            'default_shell = "/bin/bash"',
            "default_shell = " + json.dumps(str(pane_shell(root, runtime))),
        )
        config_text += (
            '\n[[keys.command]]\nkey = "prefix+p"\ntype = "popup"\n'
            'width = "90%"\nheight = "85%"\ndescription = "AW Provider details"\n'
            f"command = {json.dumps(panel_command)}\n"
        )
        (runtime / "config.toml").write_text(config_text)
        env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
        for name in ("BASH_FUNC_qoder%%", "BASH_FUNC_qodercli%%", "BASH_FUNC_codex%%"):
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
                register(root, root, pane)
                panels = Panels(root, endpoint, BINS, args.duration, log)
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
                additional_panes_seen = False
                while time.monotonic() < deadline:
                    if client.poll() is not None:
                        break
                    if server.poll() is not None:
                        raise RuntimeError("Herdr server exited unexpectedly")
                    pane_list = bridge.rpc(endpoint, "pane.list", {})["panes"]
                    panels.refresh({pane["pane_id"] for pane in pane_list})
                    pane_count = len(pane_list)
                    additional_panes_seen |= pane_count > 1
                    # Preserve the original single-pane exit behavior. Additional
                    # panes belong to Herdr and survive the first agent's exit.
                    if (
                        not additional_panes_seen
                        and all(
                            Path(entry["root"]) in panels.ended for entry in registrations(root)
                        )
                        and pane_count == 1
                    ):
                        break
                    time.sleep(0.1)
                else:
                    ownership["end_reason"] = "session duration reached"
            finally:
                signal.signal(signal.SIGINT, signal.SIG_IGN)
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                signal.signal(signal.SIGHUP, signal.SIG_IGN)
                termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, original_terminal)
                try:
                    if panels is not None:
                        panels.close()
                    else:
                        stop_agent(root)
                finally:
                    for process in (client, server):
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
                ownership.update(read(root / "ownership.json"))
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
    parser.add_argument("--attach", type=Path, help=argparse.SUPPRESS)
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
        if args.attach:
            attach(args)
        else:
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
