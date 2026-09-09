#!/usr/bin/env python3
"""Run a bounded no-auth Herdr transport/TUI smoke in an owned socket namespace."""

import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import pty
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time

from bridge import publish, rpc


def until(check, seconds: int = 10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.1)
    raise TimeoutError("Herdr smoke condition timed out")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cols", type=int, default=120)
    parser.add_argument("--rows", type=int, default=40)
    args = parser.parse_args()
    if not 80 <= args.cols <= 240 or not 24 <= args.rows <= 80:
        parser.error("smoke dimensions must be 80..240 columns and 24..80 rows")
    binary = args.binary.resolve()
    root = Path(__file__).resolve().parent
    pin = json.loads((root / "upstream.json").read_text())
    digest = hashlib.sha256(binary.read_bytes()).hexdigest()
    if digest != pin["assets"][platform.machine()]["sha256"]:
        raise ValueError("smoke requires the pinned upstream binary")
    args.output.mkdir(parents=True, exist_ok=True)
    report = {
        "version": pin["tag"],
        "binary_sha256": digest,
        "scope": "synthetic metadata / real Herdr; no Agent or AW evidence",
        "terminal_size": [args.cols, args.rows],
    }
    # Unix socket names must fit sockaddr_un, shorter than the source worktree path.
    with tempfile.TemporaryDirectory(prefix="aw-herdr-") as temporary:
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
        with (args.output / "server.log").open("wb") as log:
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
                        "command": [str(binary), "server"],
                        "stop": f"kill -TERM -- -{server.pid}",
                        "lifetime": "<= 60 seconds",
                    }
                )
                (args.output / "ownership.json").write_text(
                    json.dumps(report, indent=2)
                )
                until(lambda: rpc(endpoint, "ping", {}))
                rpc(
                    endpoint,
                    "workspace.create",
                    {"label": "AW isolated smoke", "cwd": str(runtime), "focus": True},
                )
                panes = until(lambda: rpc(endpoint, "pane.list", {}))
                if isinstance(panes, dict):
                    panes = panes["panes"]
                pane_id = panes[0]["pane_id"]
                rpc(
                    endpoint,
                    "pane.report_agent",
                    {
                        "pane_id": pane_id,
                        "source": "aw-smoke",
                        "agent": "qoder",
                        "state": "idle",
                    },
                )
                publish(
                    endpoint,
                    pane_id,
                    {
                        "aw": "AW UI fixture / no evidence",
                        "aw_sec": "SecCore: unknown",
                        "aw_tokenless": "Tokenless: unknown",
                        "aw_usage": "Ctrl+B Q: detach",
                    },
                    1,
                    1500,
                )
                pane = rpc(endpoint, "pane.get", {"pane_id": pane_id})
                assert pane["tokens"]["aw"] == "AW UI fixture / no evidence", pane
                original_pid = rpc(endpoint, "pane.process_info", {"pane_id": pane_id})[
                    "shell_pid"
                ]
                report["pane_shell_pid"] = original_pid
                report["native_metadata"] = "passed"
                master, slave = pty.openpty()
                fcntl.ioctl(
                    slave,
                    termios.TIOCSWINSZ,
                    struct.pack("HHHH", args.rows, args.cols, 0, 0),
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
                (args.output / "ownership.json").write_text(
                    json.dumps(report, indent=2)
                )
                screen = bytearray()
                deadline = time.monotonic() + 4
                while time.monotonic() < deadline:
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if readable:
                        screen.extend(os.read(master, 65536))
                    if b"AW UI fixture" in screen:
                        break
                (args.output / "screen.ansi").write_bytes(screen)
                assert b"AW UI fixture" in screen, "sidebar text absent from real TUI"
                report["sidebar_render"] = "passed"
                os.write(master, b"printf 'AW_INPUT_OK\\n'\r")
                time.sleep(0.5)
                os.write(master, b"\x02")
                time.sleep(0.1)
                os.write(master, b"q")
                deadline = time.monotonic() + 5
                while client.poll() is None and time.monotonic() < deadline:
                    readable, _, _ = select.select([master], [], [], 0.1)
                    if readable:
                        screen.extend(os.read(master, 65536))
                client.wait(timeout=1)
                (args.output / "screen.ansi").write_bytes(screen)
                report["detach"] = "passed"
                content = rpc(
                    endpoint, "pane.read", {"pane_id": pane_id, "source": "visible"}
                )
                assert (
                    "AW_INPUT_OK" in content["read"]["text"].splitlines()
                ), "shell did not produce the expected output line"
                report["terminal_input"] = "passed"
                assert (
                    rpc(endpoint, "pane.process_info", {"pane_id": pane_id})[
                        "shell_pid"
                    ]
                    == original_pid
                )
                report["pane_survives_detach"] = "passed"
                until(
                    lambda: "aw"
                    not in rpc(endpoint, "pane.get", {"pane_id": pane_id}).get(
                        "tokens", {}
                    ),
                    5,
                )
                report["metadata_expiry"] = "passed"
                (args.output / "result.json").write_text(json.dumps(report, indent=2))
                print(json.dumps(report, indent=2))
            finally:
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
                report["cleanup"] = (
                    "owned client and server exited; temporary namespace removed on return"
                )
                (args.output / "ownership.json").write_text(
                    json.dumps(report, indent=2)
                )


if __name__ == "__main__":
    main()
