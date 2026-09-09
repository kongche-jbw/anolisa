#!/usr/bin/env python3
"""Exercise two real Qoder turns through the interactive session's terminal input."""

import fcntl
import json
import os
from pathlib import Path
import pty
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
    command = [
        sys.executable,
        str(AW / "scripts/session.py"),
        "--workspace",
        str(work),
        "--allow-unrecoverable",
        "--duration",
        "180",
    ]
    process = subprocess.Popen(
        command,
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
        start_new_session=True,
    )
    ownership = {
        "command": command,
        "cwd": str(AW),
        "pid": process.pid,
        "lifetime_seconds": 180,
        "stop": f"kill -TERM {process.pid}",
        "log": str(root / "screen.ansi"),
    }
    write(root / "ownership.json", ownership)
    print(root, flush=True)
    session = None
    sent = 0
    first_session = None
    trusted = False
    failure_allowed = False
    result = {"status": "failed"}
    try:
        with (root / "screen.ansi").open("wb") as screen:
            deadline = time.monotonic() + 165
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    screen.write(os.read(master, 65536))
                    screen.flush()
                candidates = set((AW / "target/sessions").glob("*")) - before
                if len(candidates) == 1:
                    session = candidates.pop()
                if session and (session / "runtime.json").exists():
                    owned = read(session / "ownership.json")
                    pane = owned["pane_id"]
                    visible = bridge.rpc(
                        Path(owned["socket"]), "pane.read", {"pane_id": pane, "source": "visible"}
                    )
                    visible = visible.get("read", visible).get("text", "")
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
                        assert "4056" in metadata["tokens"]["aw_tokenless"], metadata
                        first_session = view["scope"]["session_id"]
                        os.write(master, b"/new")
                        time.sleep(0.2)
                        os.write(master, b"\r")
                        sent = 3
                    if sent == 3 and "Restart" in read(session / "display.json")["aw_usage"]:
                        assert not (session / "view.json").exists()
                        assert read(session / "state.json")["session_id"] != first_session
                        metadata = bridge.rpc(Path(owned["socket"]), "pane.get", {"pane_id": pane})
                        assert "unavailable" in metadata["tokens"]["aw"], metadata
                        result = {
                            "status": "passed",
                            "session": str(session),
                            "turns": 2,
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
                raise TimeoutError("two-turn live acceptance exceeded 165 seconds")
        process.wait(timeout=15)
        assert process.returncode == 0
    finally:
        if process.poll() is None:
            process.terminate()
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
