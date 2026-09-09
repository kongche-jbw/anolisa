#!/usr/bin/env python3
"""Exercise real cosh, Herdr, Codex hooks and SecCore with a local scripted model."""

import argparse
import fcntl
import json
import io
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import termios
import threading
import time
import uuid

import native_smoke

AW = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(AW / "scripts"))
from session_hooks import read, write
from session_observer import bridge


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--multipane", action="store_true")
    parser.add_argument("--untrusted", action="store_true")
    parser.add_argument("--interactive", action="store_true")
    args = parser.parse_args()
    if sum((args.multipane, args.untrusted, args.interactive)) > 1:
        parser.error("select one scenario")
    root = AW / "target" / ("codex-session-" + uuid.uuid4().hex[:8])
    root.mkdir(mode=0o700)
    home = root / "home"
    (home / ".codex").mkdir(parents=True)
    (home / "tmp").mkdir()
    work = root / "work"
    work.mkdir()
    (work / "credential.env").write_text(native_smoke.FIXTURES["sensitive"][0])
    (work / "clean.txt").write_text(native_smoke.FIXTURES["clean"][0])
    # Hold the second model response until the live sidebar is independently checked.
    release = threading.Event()
    tool_done = threading.Event()
    original = native_smoke.FixtureHandler.do_POST

    def handle(handler):
        stream = handler.rfile
        if args.interactive:
            length = int(handler.headers.get("Content-Length", "0"))
            if not 0 < length <= 2 * 1024 * 1024:
                handler.send_error(400)
                return
            raw = stream.read(length)
            payload = json.loads(raw)
            schema = payload.get("text", {}).get("format", {}).get("schema", {})
            if schema.get("required") == ["title"]:
                title_path = root / "title-request.json"
                if title_path.exists():
                    handler.server.failure = "unexpected repeated title request"
                    handler.send_error(400)
                    return
                write(title_path, payload)
                item = {
                    "type": "message",
                    "role": "assistant",
                    "id": "aw-title",
                    "status": "completed",
                    "content": [
                        {
                            "type": "output_text",
                            "text": '{"title":"Inspect synthetic credential"}',
                            "annotations": [],
                        }
                    ],
                }
                response = {
                    "id": "aw-title-response",
                    "object": "response",
                    "status": "completed",
                    "output": [item],
                }
                events = [
                    {
                        "type": "response.created",
                        "response": {**response, "status": "in_progress", "output": []},
                    },
                    {"type": "response.output_item.done", "output_index": 0, "item": item},
                    {"type": "response.completed", "response": response},
                ]
                data = "".join(
                    f"event: {event['type']}\ndata: {json.dumps(event)}\n\n" for event in events
                ).encode()
                handler.send_response(200)
                handler.send_header("Content-Type", "text/event-stream")
                handler.send_header("Content-Length", str(len(data)))
                handler.end_headers()
                handler.wfile.write(data)
                return
            handler.rfile = io.BytesIO(raw)
        try:
            if len(handler.server.requests) == 1:
                tool_done.set()
                if not release.wait(timeout=30):
                    handler.server.failure = "sidebar verification timed out"
            original(handler)
        finally:
            handler.rfile = stream

    native_smoke.FixtureHandler.do_POST = handle
    server = native_smoke.FixtureServer(root, "cat credential.env")
    thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.1})
    thread.start()
    second_root = root / "second-model"
    second_root.mkdir()
    second_server = (
        native_smoke.FixtureServer(second_root, "cat clean.txt") if args.multipane else None
    )
    second_thread = (
        threading.Thread(target=second_server.serve_forever, kwargs={"poll_interval": 0.1})
        if second_server
        else None
    )
    if second_thread:
        second_thread.start()
    (home / ".codex/config.toml").write_text(
        'model="gpt-5.5"\nmodel_provider="aw_fixture"\napproval_policy="never"\n'
        'sandbox_mode="danger-full-access"\n'
        '[model_providers.aw_fixture]\nname="AW fixture"\n'
        f'base_url="http://127.0.0.1:{server.server_port}/v1"\nwire_api="responses"\n'
        "requires_openai_auth=false\nsupports_websockets=false\nrequest_max_retries=0\n"
        "stream_max_retries=0\nstream_idle_timeout_ms=45000\n"
        "[features]\napps=false\nplugins=false\n"
        + f'[projects.{json.dumps(str(work))}]\ntrust_level="trusted"\n'
    )
    env = native_smoke.isolated_environment(home)
    env.update(
        TMPDIR="/tmp",
        COSH_AUDIT_DIR=str(root / "cosh-audit"),
        COSH_SHELL_BOOTSTRAP_PATH="0",
        TERM="xterm-256color",
    )
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
    before = set((AW / "target/sessions").iterdir())
    command = [str(AW / "scripts/cosh"), "--isolated"]
    process = subprocess.Popen(
        command, cwd=work, env=env, stdin=slave, stdout=slave, stderr=slave, start_new_session=True
    )
    owner = {
        "command": command,
        "cwd": str(work),
        "pid": process.pid,
        "stop": f"kill -TERM -- -{process.pid}",
        "lifetime_seconds": 100,
        "server_pid": os.getpid(),
        "ports": [server.server_port] + ([second_server.server_port] if second_server else []),
        "server_stop": f"kill -TERM {os.getpid()}",
        "log": str(root / "screen.ansi"),
        "authentication": "none; isolated HOME and CODEX_HOME",
    }
    write(root / "ownership.json", owner)
    print(root, flush=True)
    session = child = None
    split = launched_second = requested_exit = False
    result = {"status": "failed"}
    try:
        time.sleep(1)
        # Bypass only the isolated fixture's vetted hooks; production never adds this flag.
        agent_command = [
            "codex",
            "exec",
            "--ephemeral",
            "--ignore-rules",
            "--skip-git-repo-check",
            "--dangerously-bypass-hook-trust",
            "--json",
            "Read credential.env using the shell and finish; this is a synthetic scanner test.",
        ]
        if args.interactive:
            agent_command = ["codex", "--dangerously-bypass-hook-trust", agent_command[-1]]
        if args.untrusted:
            agent_command.remove("--dangerously-bypass-hook-trust")
        owner["agent_command"] = agent_command
        write(root / "ownership.json", owner)
        os.write(master, (shlex.join(agent_command) + "\r").encode())
        deadline = time.monotonic() + 75
        verified = None
        with (root / "screen.ansi").open("wb") as log:
            while time.monotonic() < deadline:
                ready, _, _ = select.select([master], [], [], 0.1)
                if ready:
                    log.write(os.read(master, 65536))
                    log.flush()
                created = set((AW / "target/sessions").iterdir()) - before
                if len(created) == 1:
                    session = next(iter(created)) if session is None else session
                if (
                    args.interactive
                    and session
                    and (session / "runtime.json").exists()
                    and not requested_exit
                ):
                    ownership = read(session / "ownership.json")
                    if (
                        "socket" in ownership
                        and "pane_id" in ownership
                        and "cleanup" not in ownership
                    ):
                        visible = bridge.rpc(
                            Path(ownership["socket"]),
                            "pane.read",
                            {"pane_id": ownership["pane_id"], "source": "visible"},
                        )
                        write(root / "visible.json", visible)
                    if (
                        release.is_set()
                        and len(server.requests) == 2
                        and not requested_exit
                        and "AW hook smoke completed" in str(visible)
                    ):
                        os.write(master, b"\x02q")
                        requested_exit = True
                if args.untrusted and session and tool_done.is_set() and verified is None:
                    assert read(session / "state.json")["session_id"] is None
                    assert not list((session / "evidence").glob("*.json"))
                    assert "waiting" in read(session / "display.json")["aw"]
                    verified = {
                        "status": "passed",
                        "scope": "untrusted hooks skipped; no Provider calls claimed",
                    }
                    release.set()
                if session and (session / "view.json").exists() and verified is None:
                    view = read(session / "view.json")
                    if view["events"]:
                        details = read(session / "provider-details.json")
                        assert details["status"] == "verified" and details["bash_results"] == 1
                        assert details["recent_calls"][0]["verdict"] == "sensitive"
                        assert any(
                            f["rule_id"] == "api_key"
                            for f in details["recent_calls"][0]["findings"]
                        )
                        assert [p["provider_id"] for p in view["providers"]] == ["sec-core"]
                        assert view["adoption"] == "not_observed"
                        ownership = read(session / "ownership.json")
                        display = read(session / "display.json")
                        assert "unsupported" in display["aw_tokenless"]
                        entry = next((session / "panels").glob("*.json"))
                        pane = bridge.rpc(
                            Path(ownership["socket"]), "pane.get", {"pane_id": read(entry)["pane"]}
                        )
                        write(root / "verified-pane.json", pane)
                        write(root / "verified-details.json", details)
                        verified = native_smoke.validate_inspection(session, "sensitive")
                        if args.multipane:
                            os.write(master, b"\x02v")
                            split = True
                        else:
                            release.set()
                if split and not launched_second:
                    ownership = read(session / "ownership.json")
                    endpoint = Path(ownership["socket"])
                    pane_id = bridge.rpc(endpoint, "pane.current", {})["pane"]["pane_id"]
                    first_pane = read(next((session / "panels").glob("*.json")))["pane"]
                    if pane_id != first_pane:
                        time.sleep(1)
                        second_command = [
                            *agent_command[:-1],
                            "-c",
                            f'model_providers.aw_fixture.base_url="http://127.0.0.1:{second_server.server_port}/v1"',
                            "Read clean.txt using the shell and finish.",
                        ]
                        owner["second_command"] = second_command
                        write(root / "ownership.json", owner)
                        os.write(master, (shlex.join(second_command) + "\r").encode())
                        launched_second = True
                if launched_second and not release.is_set():
                    entries = [read(p) for p in (session / "panels").glob("*.json")]
                    children = [Path(e["root"]) for e in entries if e["root"] != str(session)]
                    if len(children) == 1:
                        child = children[0]
                        if (child / "view.json").exists() and read(child / "view.json")["events"]:
                            details = read(child / "provider-details.json")
                            assert (
                                details["bash_results"] == 1
                                and details["recent_calls"][0]["verdict"] == "clean"
                            )
                            assert (
                                read(session / "view.json")["scope"]["session_id"]
                                != details["session_id"]
                            )
                            assert (
                                read(session / "provider-details.json")["recent_calls"][0][
                                    "verdict"
                                ]
                                == "sensitive"
                            )
                            native_smoke.validate_inspection(child, "clean")
                            write(root / "second-verified-details.json", details)
                            os.write(master, b"\x02p")
                            popup_deadline = time.monotonic() + 5
                            while time.monotonic() < popup_deadline:
                                if list(child.glob("provider-panel-*.json")):
                                    break
                                time.sleep(0.1)
                            else:
                                raise TimeoutError(
                                    "Provider popup did not target second Codex pane"
                                )
                            assert not list(session.glob("provider-panel-*.json"))
                            os.write(master, b"q")
                            release.set()
                if (
                    child
                    and release.is_set()
                    and not requested_exit
                    and read(child / "provider-details.json")["status"] == "agent exited"
                ):
                    os.write(master, b"\x02q")
                    requested_exit = True
                if (
                    session
                    and (session / "ownership.json").exists()
                    and "cleanup" in read(session / "ownership.json")
                ):
                    assert verified is not None, "Codex exited before verified sidebar evidence"
                    assert not server.failure and len(server.requests) == 2
                    outputs = [
                        r
                        for r in server.requests[1].get("input", [])
                        if r.get("type") == "function_call_output"
                    ]
                    assert any("api_key=sk-" in str(r.get("output")) for r in outputs)
                    os.write(master, b"exit\r")
                    process.wait(timeout=10)
                    assert process.returncode == 0
                    result = {
                        "status": "passed",
                        "session": str(session),
                        "second_session": str(child) if child else None,
                        "inspection": verified,
                        "scope": "real cosh/Herdr/Codex; local scripted model; "
                        + (
                            "untrusted hooks skipped"
                            if args.untrusted
                            else "real SecCore inspection, original output retained"
                        ),
                    }
                    break
            else:
                raise TimeoutError("Codex AW sidebar did not verify within 75 seconds")
    finally:
        release.set()
        if process.poll() is None:
            if session and (session / "ownership.json").exists():
                ownership = read(session / "ownership.json")
                if "cleanup" not in ownership:
                    os.kill(ownership["launcher_pid"], signal.SIGTERM)
            os.write(master, b"exit\r")
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=5)
        if second_server:
            second_server.shutdown()
            second_server.server_close()
            second_thread.join(timeout=5)
            assert not second_thread.is_alive()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        os.close(master)
        os.close(slave)
        assert not thread.is_alive()
        assert not Path(f"/proc/{process.pid}").exists()
        if session and (session / "ownership.json").exists():
            ownership = read(session / "ownership.json")
            assert not Path(ownership["temporary_directory"]).exists()
            for key in ("server_pid", "client_pid", "observer_pid"):
                if key in ownership:
                    assert not Path(f"/proc/{ownership[key]}").exists()
            if (session / "runtime.json").exists():
                assert not Path(f"/proc/{read(session / 'runtime.json')['agent_pid']}").exists()
        if child:
            child_owner = read(child / "ownership.json")
            for record in child.glob("provider-panel-*.json"):
                assert not Path(f"/proc/{read(record)['pid']}").exists()
            for key in ("observer_pid",):
                if key in child_owner:
                    assert not Path(f"/proc/{child_owner[key]}").exists()
            assert not Path(f"/proc/{read(child / 'runtime.json')['agent_pid']}").exists()
        if args.interactive:
            # The TUI persists native session state; verify isolation and remove it
            # with the test home instead of leaving a user-owned conversation.
            owner["isolated_native_history_files"] = len(list((home / ".codex").rglob("*.jsonl")))
            write(root / "ownership.json", owner)
        shutil.rmtree(home)
        assert not home.exists()
        result["cleanup"] = (
            "owned processes, socket namespace and isolated home absent; evidence retained"
        )
        write(root / "result.json", result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
