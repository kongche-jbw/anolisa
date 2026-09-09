"""Own per-pane agent registrations and observers inside one private Herdr server."""

import os
from pathlib import Path
import shlex
import signal
import subprocess
import sys
import time

from session_hooks import read, write
from session_observer import bridge


def pane_shell(group: Path, temporary: Path) -> Path:
    """Install instance-local Bash commands after the user's normal interactive rc."""
    entry = Path(__file__).with_name("session.py")
    rc = temporary / "pane.bashrc"
    command = shlex.join([sys.executable, str(entry), "--attach", str(group)])
    rc.write_text(
        "[[ -f ~/.bashrc ]] && source ~/.bashrc\n"
        "unalias qoder qodercli codex 2>/dev/null || true\n"
        f'qoder() {{ {command} --agent-kind qoder -- "$@"; }}\n'
        'qodercli() { qoder "$@"; }\n'
        f'codex() {{ {command} --agent-kind codex -- "$@"; }}\n'
    )
    shell = temporary / "pane-shell"
    shell.write_text("#!/bin/bash\nexec /bin/bash --rcfile " + shlex.quote(str(rc)) + " -i\n")
    shell.chmod(0o700)
    return shell


def register(group: Path, root: Path, pane: str) -> None:
    write(group / "panels" / f"{root.name}.json", {"root": str(root), "pane": pane})


def registrations(group: Path) -> list[dict]:
    entries = []
    for path in (group / "panels").glob("*.json"):
        entry = read(path)
        root = Path(entry["root"])
        if root.parent != group.parent or root.name != path.stem:
            raise ValueError("pane registration is outside the owned session directory")
        entries.append(entry)
    return entries


def stop_observer(process: subprocess.Popen) -> None:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


class Panels:
    def __init__(self, group: Path, endpoint: Path, bins: Path, duration: int, log):
        self.group, self.endpoint, self.bins = group, endpoint, bins
        self.duration, self.log = duration, log
        self.observers = {}
        self.ended = set()
        self.next_codex_update = {}

    def refresh(self, present_panes: set[str]) -> None:
        from session import live, stop_agent

        for entry in registrations(self.group):
            root, pane = Path(entry["root"]), entry["pane"]
            runtime_path = root / "runtime.json"
            if not runtime_path.exists() or root in self.ended:
                continue
            config = read(runtime_path)
            if pane not in present_panes or not live(
                config["agent_pid"], config["agent_start_ticks"]
            ):
                if root in self.observers:
                    stop_observer(self.observers.pop(root))
                stop_agent(root)
                write(root / "provider-details.json", {"status": "agent exited"})
                self.ended.add(root)
                continue
            if config["agent_kind"] == "codex":
                if time.monotonic() >= self.next_codex_update.get(root, 0):
                    bridge.publish(
                        self.endpoint,
                        pane,
                        {
                            **{key: None for key in bridge.TOKEN_NAMES},
                            "aw": "AW hooks: not connected",
                            "aw_sec": "SecCore: not invoked",
                            "aw_tokenless": "Tokenless: not invoked",
                            "aw_usage": "Ctrl+B p: Provider configuration",
                        },
                        time.monotonic_ns(),
                    )
                    self.next_codex_update[root] = time.monotonic() + 2
                continue
            if root not in self.observers:
                command = [
                    sys.executable,
                    str(Path(__file__).with_name("session_observer.py")),
                    str(root),
                    str(self.endpoint),
                    pane,
                    str(self.bins),
                    str(self.duration),
                ]
                process = subprocess.Popen(
                    command,
                    stdout=self.log,
                    stderr=self.log,
                    env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
                    start_new_session=True,
                )
                self.observers[root] = process
                ownership = read(root / "ownership.json")
                ownership.update(
                    observer_pid=process.pid,
                    observer_command=command,
                    observer_stop=f"kill -TERM -- -{process.pid}",
                )
                write(root / "ownership.json", ownership)
            elif self.observers[root].poll() is not None:
                raise RuntimeError(f"AW observer exited for pane {pane}; inspect herdr.log")

    def close(self) -> None:
        from session import stop_agent

        for process in self.observers.values():
            stop_observer(process)
        for entry in registrations(self.group):
            root = Path(entry["root"])
            if root in self.ended:
                continue
            stop_agent(root)
            if root != self.group:
                ownership = read(root / "ownership.json")
                ownership["cleanup"] = "owned agent and observer stopped by Herdr launcher"
                write(root / "ownership.json", ownership)


def focused_root(group: Path) -> Path:
    """Resolve the popup target from native focus, never from the first agent."""
    from session import live

    endpoint = Path(read(group / "ownership.json")["socket"])
    pane = bridge.rpc(endpoint, "pane.current", {})["pane"]["pane_id"]
    for entry in registrations(group):
        root = Path(entry["root"])
        if entry["pane"] != pane or not (root / "runtime.json").exists():
            continue
        runtime = read(root / "runtime.json")
        if live(runtime["agent_pid"], runtime["agent_start_ticks"]):
            return root
    raise ValueError(f"No AW agent is attached to focused pane {pane}")
