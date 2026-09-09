#!/usr/bin/env python3
"""Run an authorized real Agent acceptance command in an isolated Herdr pane."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import pty
import re
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

from bridge import rpc
from smoke import until


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--display",
        action="store_true",
        help="mirror the real Herdr TUI to this terminal",
    )
    parser.add_argument(
        "--hold-seconds", type=int, choices=range(1, 121), default=3, metavar="1..120"
    )
    parser.add_argument(
        "--command-json",
        type=Path,
        required=True,
        help="trusted JSON argv for the AW Tokenless acceptance runner",
    )
    args = parser.parse_args()
    if args.display and not sys.stdout.isatty():
        parser.error("--display requires a terminal")
    signal.signal(signal.SIGTERM, lambda _signal, _frame: sys.exit(130))
    binary = args.binary.resolve()
    root = Path(__file__).resolve().parent
    pin = json.loads((root / "upstream.json").read_text())
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    if digest != pin["assets"][platform.machine()]["sha256"]:
        raise ValueError("live smoke requires the pinned upstream binary")
    command = json.loads(args.command_json.read_text())
    if (
        not isinstance(command, list)
        or not command
        or not all(isinstance(arg, str) and "\x00" not in arg for arg in command)
    ):
        raise ValueError("command must be a trusted nonempty JSON argv")
    evidence = Path(command[command.index("--output-dir") + 1]).resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    output = args.output.resolve()
    if evidence.exists():
        raise ValueError("live acceptance output directory must not already exist")
    report = {
        "version": pin["tag"],
        "binary_sha256": digest,
        "scope": "real Agent command in pinned Herdr",
        "evidence": str(evidence),
    }
    with tempfile.TemporaryDirectory(prefix="aw-herdr-live-") as temporary:
        runtime = Path(temporary)
        config = runtime / "config.toml"
        shutil.copyfile(root / "config.toml", config)
        endpoint = runtime / "api.sock"
        env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("HERDR_")
        }
        env.update(
            {
                "HERDR_SOCKET_PATH": str(endpoint),
                "HERDR_CONFIG_PATH": str(config),
                "XDG_CONFIG_HOME": str(runtime / "config"),
                "XDG_STATE_HOME": str(runtime / "state"),
                "TERM": "xterm-256color",
            }
        )
        server = client = None
        master = slave = None
        screen = bytearray()
        with (output / "server.log").open("wb") as log:
            try:
                server = subprocess.Popen(
                    [str(binary), "server"],
                    cwd=runtime,
                    env=env,
                    stdout=log,
                    stderr=log,
                    start_new_session=True,
                )
                report.update(
                    {
                        "server_pid": server.pid,
                        "socket": str(endpoint),
                        "cwd": str(runtime),
                        "server_command": [str(binary), "server"],
                        "stop": f"kill -TERM -- -{server.pid}",
                        "lifetime": f"<= {210 + args.hold_seconds} seconds",
                    }
                )
                (output / "ownership.json").write_text(json.dumps(report, indent=2))
                until(lambda: rpc(endpoint, "ping", {}))
                rpc(
                    endpoint,
                    "workspace.create",
                    {"label": "AW live acceptance", "cwd": str(runtime), "focus": True},
                )
                pane_id = rpc(endpoint, "pane.list", {})["panes"][0]["pane_id"]
                command.extend(
                    [
                        "--interactive",
                        "--herdr-socket",
                        str(endpoint),
                        "--herdr-pane-id",
                        pane_id,
                        "--hold-seconds",
                        str(args.hold_seconds),
                    ]
                )
                report["agent_command"] = command
                master, slave = pty.openpty()
                fcntl.ioctl(
                    slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0)
                )
                client = subprocess.Popen(
                    [str(binary)],
                    cwd=runtime,
                    env=env,
                    stdin=slave,
                    stdout=slave,
                    stderr=slave,
                    start_new_session=True,
                )
                report["client_pid"] = client.pid
                (output / "ownership.json").write_text(json.dumps(report, indent=2))
                status = runtime / "command-status"
                command_log = output / "command.log"
                shell = (
                    shlex.join(command)
                    + " >"
                    + shlex.quote(str(command_log))
                    + " 2>&1; printf '%s' \"$?\" >"
                    + shlex.quote(str(status))
                    + "\n"
                )
                rpc(endpoint, "pane.send_input", {"pane_id": pane_id, "text": shell})
                deadline = time.monotonic() + 180 + args.hold_seconds
                finished_at = None
                while time.monotonic() < deadline:
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if readable:
                        chunk = os.read(master, 65536)
                        screen.extend(chunk)
                        if args.display:
                            sys.stdout.buffer.write(chunk)
                            sys.stdout.buffer.flush()
                    if status.exists():
                        finished_at = finished_at or time.monotonic()
                        if time.monotonic() - finished_at > 0.5:
                            break
                    if client.poll() is not None or server.poll() is not None:
                        raise RuntimeError(
                            "Herdr exited before live acceptance completed"
                        )
                if not status.exists():
                    raise TimeoutError("live Agent acceptance exceeded 180 seconds")
                if status.read_text().strip() != "0":
                    raise RuntimeError("live Agent acceptance failed; see command.log")
                view = json.loads((evidence / "herdr-view.json").read_text())
                metadata = json.loads((evidence / "herdr-metadata.json").read_text())
                if metadata.get("pane"):
                    metadata = metadata["pane"]
                provider = next(
                    item for item in view["providers"] if item["kind"] == "projection"
                )
                if (
                    view["verification"] != "journal_verified"
                    or view["adoption"] != "local_history"
                    or provider["adopted"] <= 0
                    or provider["saved_bytes"] <= 0
                ):
                    raise ValueError(
                        "live acceptance did not produce verified history adoption"
                    )
                expected = f'-{provider["saved_bytes"]} B'
                text = re.sub(rb"\x1b\[[0-?]*[ -/]*[@-~]", b"", bytes(screen)).decode(
                    errors="replace"
                )
                if "history adopted" not in text or expected not in text:
                    raise ValueError(
                        "verified adoption text absent from real Herdr TUI"
                    )
                if "history adopted" not in metadata["tokens"]["aw_tokenless"]:
                    raise ValueError(
                        "verified adoption text absent from native metadata"
                    )
                report.update(
                    {
                        "status": "passed",
                        "pane_id": pane_id,
                        "adopted": provider["adopted"],
                        "saved_bytes": provider["saved_bytes"],
                        "adoption_boundary": "local_history",
                        "real_sidebar_render": "passed",
                    }
                )
                (output / "result.json").write_text(json.dumps(report, indent=2))
                if not args.display:
                    print(json.dumps(report, indent=2))
            finally:
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
                signal.signal(signal.SIGINT, signal.SIG_IGN)
                (output / "screen.ansi").write_bytes(screen)
                # Give the acceptance runner time to remove its own Qoder session/trust.
                lifecycle_path = evidence / "lifecycle.json"
                if lifecycle_path.is_file():
                    lifecycle = json.loads(lifecycle_path.read_text())
                    pid = lifecycle.get("runner_pid")
                    stat = Path(f"/proc/{pid}/stat")
                    if pid and stat.exists():
                        try:
                            ticks = int(stat.read_text().rsplit(")", 1)[1].split()[19])
                            if ticks == lifecycle.get("runner_start_ticks"):
                                os.kill(pid, signal.SIGTERM)
                                end = time.monotonic() + 8
                                while stat.exists() and time.monotonic() < end:
                                    time.sleep(0.1)
                        except (ProcessLookupError, FileNotFoundError):
                            pass
                for process in (client, server):
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
                if args.display:
                    sys.stdout.write(
                        "\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l"
                        "\x1b[?1015l\x1b[?2004l\x1b[?1004l\x1b[?2031l"
                        "\x1b[<u\x1b[?7h\x1b[?1049l\x1b[?25h\x1b[0m"
                    )
                    sys.stdout.flush()
                report["cleanup"] = (
                    "owned client/server exited; temporary namespace removed on return"
                )
                (output / "ownership.json").write_text(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
