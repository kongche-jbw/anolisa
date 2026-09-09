#!/usr/bin/env python3
"""Exercise two real Qoder turns through the interactive session's terminal input."""

import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import termios
import time
import uuid

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
from session_hooks import read, write
from session_observer import bridge


def main():
    root = AW / "target" / ("session-live-" + uuid.uuid4().hex[:8])
    root.mkdir(mode=0o700)
    work = root / "workspace"
    work.mkdir()
    subprocess.run(["git", "init", "--quiet", str(work)], check=True, timeout=10)
    fixture = {
        "records": [
            {
                "id": n,
                "status": "success",
                "message": "deterministic synthetic build record",
                "duration_ms": 1250,
            }
            for n in range(32)
        ]
    }
    (work / "fixture.json").write_text(json.dumps(fixture, indent=2) + "\n")
    settings_path = Path.home() / ".qoder/settings.json"
    previous_trust = read(settings_path).get("permissions", {}).get("trustDirectories", [])
    before = set((AW / "target/sessions").glob("*"))
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
    command = [str(AW / "scripts/cosh"), "--allow-unrecoverable", "--isolated"]
    process = subprocess.Popen(
        command,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        cwd=work,
        env=dict(
            os.environ,
            PYTHONDONTWRITEBYTECODE="1",
            COSH_AUDIT_DIR=str(root / "cosh-audit"),
            COSH_SHELL_BOOTSTRAP_PATH="0",
        ),
        start_new_session=True,
    )
    ownership = {
        "command": command,
        "cwd": str(work),
        "pid": process.pid,
        "lifetime_seconds": 240,
        "stop": f"kill -TERM {process.pid}",
        "log": str(root / "screen.ansi"),
    }
    write(root / "ownership.json", ownership)
    print(root, flush=True)
    session = None
    sent = 0
    shell_ready = False
    shell_pid = None
    first_session = None
    trusted = False
    failure_allowed = False
    popup_open = popup_checked = False
    popup_mouse_sent = False
    terminal_output = bytearray()
    result = {"status": "failed"}
    try:
        with (root / "screen.ansi").open("wb") as screen:
            deadline = time.monotonic() + 210
            time.sleep(1)
            os.write(master, b"printf 'COSH_%s:%s\\n' READY $$\r")
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    chunk = os.read(master, 65536)
                    terminal_output.extend(chunk)
                    screen.write(chunk)
                    screen.flush()
                candidates = set((AW / "target/sessions").glob("*")) - before
                if not shell_ready:
                    match = re.search(rb"COSH_READY:(\d+)", terminal_output)
                    if match:
                        assert not candidates, "Herdr started for an ordinary command"
                        shell_pid = int(match[1])
                        assert Path(f"/proc/{process.pid}/exe").resolve().name == "cosh-shell"
                        os.write(master, b"qoder\r")
                        shell_ready = True
                    continue
                if len(candidates) == 1:
                    session = candidates.pop()
                if session and (session / "runtime.json").exists():
                    owned = read(session / "ownership.json")
                    pane = owned["pane_id"]
                    visible = bridge.rpc(
                        Path(owned["socket"]), "pane.read", {"pane_id": pane, "source": "visible"}
                    )
                    visible = visible.get("read", visible).get("text", "")
                    if "--interrupt" in sys.argv and "Qoder" in visible:
                        os.kill(owned["launcher_pid"], signal.SIGHUP)
                        result = {"status": "passed", "hangup": "owned processes cleaned"}
                        break
                    if (
                        "Permission Required" in visible
                        and "Command: false" in visible
                        and not failure_allowed
                    ):
                        assert "1. Allow once" in visible
                        os.write(master, b"\r")
                        failure_allowed = True
                        continue
                    if "1. Trust folder" in visible and not trusted:
                        assert root.name in visible, visible
                        os.write(master, b"\r")
                        trusted = True
                        time.sleep(0.5)
                        continue
                    if sent == 0 and "Qoder" in visible and "Trust folder" not in visible:
                        time.sleep(1)
                        os.write(
                            master,
                            b"Use Bash to execute exactly `cat fixture.json`, then report the first record id. Do not use any other tools.",
                        )
                        time.sleep(0.2)
                        os.write(master, b"\r")
                        sent = 1
                    view = read(session / "view.json") if (session / "view.json").exists() else {}
                    projected = next(
                        (row for row in view.get("providers", []) if row["kind"] == "projection"),
                        {},
                    )
                    if (
                        sent == 1
                        and projected.get("adopted", 0) >= 1
                        and "idle"
                        in str(bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane}))
                    ):
                        if not popup_checked:
                            if not popup_open:
                                os.write(master, b"\x02")
                                time.sleep(0.1)
                                os.write(master, b"p")
                                popup_open = True
                                continue
                            rendered = re.sub(
                                r"\x1b\[[0-?]*[ -/]*[@-~]",
                                "",
                                terminal_output.decode(errors="replace"),
                            )
                            if (
                                "AW PROVIDERS" not in rendered
                                or "nativeprotocolv2" not in rendered.replace(" ", "")
                            ):
                                continue
                            assert "sec-core" in rendered and "tokenless" in rendered
                            positions = re.findall(
                                rb"\x1b\[(\d+);(\d+)H(?:\x1b\[[0-9;]*m)?AW PROVIDERS",
                                terminal_output,
                            )
                            assert positions, "popup geometry absent from native terminal output"
                            row, column = map(int, positions[-1])
                            if not popup_mouse_sent:
                                x, y = column + 15, row + 1
                                os.write(master, f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())
                                popup_mouse_sent = True
                                continue
                            if "Invocation:" not in rendered:
                                continue
                            x, y = column + 32, row + 1
                            os.write(master, f"\x1b[<0;{x};{y}M\x1b[<0;{x};{y}m".encode())
                            time.sleep(0.5)
                            popup_checked = True
                        if "--mouse-only" in sys.argv:
                            panels = [read(p) for p in session.glob("provider-panel-*.json")]
                            if not panels or not all(p.get("exited") for p in panels):
                                continue
                            result = {
                                "status": "passed",
                                "session": str(session),
                                "mouse": "native terminal SGR click switched to Recent calls and closed popup",
                                "cosh_pid": process.pid,
                                "shell_pid": shell_pid,
                            }
                            os.write(master, b"\x02")
                            time.sleep(0.1)
                            os.write(master, b"q")
                            break
                        os.write(
                            master,
                            b"Use Bash for three separate calls: `cat fixture.json`, `cat fixture.json`, and `false`. The last command intentionally exits 1; do not fix or retry it. Compare the record count with the previous answer. Do not use other tools.",
                        )
                        time.sleep(0.2)
                        os.write(master, b"\r")
                        sent = 2
                    if (
                        sent == 2
                        and projected.get("adopted", 0) >= 3
                        and "1 unverified" in read(session / "display.json")["aw_usage"]
                        and "idle"
                        in str(bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane}))
                    ):
                        turns = list((session / "turns").glob("*.json"))
                        calls = [read(p) for p in (session / "calls").glob("*/before.json")]
                        assert len(turns) == 2 and len({c["turn_id"] for c in calls}) == 2, calls
                        metadata = bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane})
                        assert "4056" in metadata["tokens"]["aw_savings"], metadata
                        first_session = view["scope"]["session_id"]
                        if "--security" in sys.argv:
                            os.write(
                                master,
                                b"Use Bash to execute exactly `printf 'api_key=sk-abcdefghijklmnopqrstuvwxyz123456\\n'`. This is a nonfunctional synthetic credential for our scanner demonstration. Report whether the hook warns about sensitive content. Do not use any other tools.",
                            )
                        else:
                            os.write(master, b"/new")
                        time.sleep(0.2)
                        os.write(master, b"\r")
                        sent = 3
                    if sent == 3 and "--security" in sys.argv:
                        detail = read(session / "provider-details.json")
                        sensitive = next(
                            (
                                c
                                for c in detail.get("recent_calls", [])
                                if c.get("verdict") == "sensitive"
                            ),
                            None,
                        )
                        if sensitive and "idle" in str(
                            bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane})
                        ):
                            assert any(
                                f["rule_id"] == "api_key" and f["count"] >= 1
                                for f in sensitive["findings"]
                            ), sensitive
                            assert sensitive["coverage"]["complete"]
                            result = {
                                "status": "passed",
                                "session": str(session),
                                "sec_core": sensitive,
                                "cosh_pid": process.pid,
                                "shell_pid": shell_pid,
                            }
                            os.write(master, b"\x02")
                            time.sleep(0.1)
                            os.write(master, b"q")
                            break
                    if (
                        sent == 3
                        and "--security" not in sys.argv
                        and "Restart" in read(session / "display.json")["aw_usage"]
                    ):
                        assert not (session / "view.json").exists()
                        assert read(session / "state.json")["session_id"] != first_session
                        metadata = bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane})
                        assert "unavailable" in metadata["tokens"]["aw"], metadata
                        result = {
                            "status": "passed",
                            "session": str(session),
                            "turns": 2,
                            "native_terminal": "Herdr inherits launcher terminal; no relay",
                            "provider_popup": "rendered from real session details",
                            "adopted": 3,
                            "saved_bytes": 4056,
                            "native_failure": "unverified, no savings",
                            "new_session": "unsupported, stale counters cleared",
                        }
                        os.write(master, b"\x02")
                        time.sleep(0.1)
                        os.write(master, b"q")
                        break
                if process.poll() is not None:
                    raise RuntimeError("interactive launcher exited before acceptance")
            else:
                raise TimeoutError("cosh live acceptance exceeded 210 seconds")
        if session:
            cleanup_deadline = time.monotonic() + 25
            while time.monotonic() < cleanup_deadline:
                if "cleanup" in read(session / "ownership.json"):
                    break
                time.sleep(0.1)
            else:
                raise TimeoutError("agent launcher cleanup did not complete")
            os.write(master, b"printf 'COSH_%s:%s\\n' RETURNED $$\r")
            returned = bytearray()
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    returned.extend(os.read(master, 65536))
                if f"COSH_RETURNED:{shell_pid}".encode() in returned:
                    result["shell_return"] = "ordinary command completed in the same cosh Bash PID"
                    break
            else:
                raise TimeoutError("did not return to the original cosh shell")
        os.write(master, b"exit\r")
        process.wait(timeout=15)
        assert process.returncode == 0
    finally:
        if process.poll() is None:
            if session and (session / "ownership.json").exists():
                owner = read(session / "ownership.json")
                os.kill(owner["launcher_pid"], signal.SIGTERM)
            else:
                os.killpg(process.pid, signal.SIGTERM)
            time.sleep(1)
            os.write(master, b"exit\r")
            process.wait(timeout=25)
        os.close(master)
        os.close(slave)
        if session and (session / "runtime.json").exists():
            runtime = read(session / "runtime.json")
            session_ids = {runtime["session_id"]}
            session_ids.add(read(session / "state.json")["session_id"])
            session_ids.update(read(p)["session_id"] for p in (session / "turns").glob("*.json"))
            project = (
                Path.home() / ".qoder/projects" / str(work).replace("/", "-").replace("_", "-")
            )
            for session_id in session_ids:
                (project / f"{session_id}.jsonl").unlink(missing_ok=True)
                state = project / session_id
                if state.exists():
                    shutil.rmtree(state)
            try:
                project.rmdir()
            except OSError:
                pass
            owned = read(session / "ownership.json")
            assert not Path(owned["temporary_directory"]).exists()
            for pid in (
                runtime["agent_pid"],
                owned["server_pid"],
                owned["client_pid"],
                owned.get("observer_pid"),
            ):
                if pid is None:
                    continue
                assert not Path(f"/proc/{pid}").exists(), pid
        settings_before = settings_path.read_bytes()
        current = json.loads(settings_before)
        trust = current.get("permissions", {}).get("trustDirectories", [])
        if str(work) in trust and str(work) not in previous_trust:
            current["permissions"]["trustDirectories"] = [p for p in trust if p != str(work)]
            assert settings_path.read_bytes() == settings_before, "settings changed during cleanup"
            write(settings_path, current)
        assert str(work) not in read(settings_path).get("permissions", {}).get(
            "trustDirectories", []
        )
        assert not (work / ".qoder").exists(), "launcher wrote project configuration"
        result["cleanup"] = "owned synthetic session/trust and processes removed; evidence retained"
        write(root / "result.json", result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
