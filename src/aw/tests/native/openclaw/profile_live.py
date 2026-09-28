#!/usr/bin/env python3
"""Bound two real OpenClaw Gateway launches against one owned persistent profile."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import time
from typing import Any
import uuid


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--key-file", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[5]
    root = (args.output or repo / "target/profile-lab/openclaw").resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    os.umask(0o077)
    state, profile, workspace = root / "aw-state", root / "profile", root / "workspace"
    plugin = profile / "extensions/existing-tool-hooks"
    for directory in (state, plugin, workspace, root / "home"):
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    key = args.key_file.read_text().strip()
    assert key, "A private credential file is required"
    aw = repo / "src/aw/target/debug/aw"
    runtime = repo / "target/native-lab/openclaw/runtime/node_modules"
    node, package = runtime / "node/bin/node", runtime / "openclaw"
    cli = package / "openclaw.mjs"
    endpoint = state / "aw.sock"
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    native = profile / "openclaw.json"
    environment = {name: value for name, value in os.environ.items()
                   if not name.startswith(("OPENCLAW_", "AW_READY_")) and key not in value
                   and name not in {"DASHSCOPE_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY"}}
    environment.update(OPENCLAW_HOME=str(root / "home"), OPENCLAW_STATE_DIR=str(profile),
                       OPENCLAW_CONFIG_PATH=str(native), OPENCLAW_SKIP_CHANNELS="1")
    deadline = time.monotonic() + 480
    records: list[dict[str, Any]] = []
    children: list[tuple[subprocess.Popen[bytes], dict[str, Any]]] = []
    daemon_pid = None
    result: dict[str, Any] = {"status": "started", "framework": "OpenClaw 2026.9.6",
                              "profile": str(profile), "rounds": []}

    def write(path: Path, value: Any) -> None:
        path.write_text(json.dumps(value, indent=2) + "\n")

    def save() -> None:
        write(root / "processes.json", records)

    def start(name: str, argv: list[Any], seconds: int) -> subprocess.Popen[bytes]:
        record = {"name": name, "command": list(map(str, argv)), "cwd": str(repo),
                  "log": str(root / f"{name}.log"), "ports": [port] if "gateway" in name else [],
                  "pid": None, "timeout_seconds": seconds, "started": time.time()}
        records.append(record)
        save()
        with Path(record["log"]).open("wb") as output:
            child = subprocess.Popen(record["command"], cwd=repo, env=environment,
                                     stdin=subprocess.DEVNULL, stdout=output, stderr=subprocess.STDOUT,
                                     start_new_session=True)
        record.update(pid=child.pid, pgid=child.pid, stop=f"kill -TERM -- -{child.pid}")
        children.append((child, record))
        save()
        return child

    def run(name: str, argv: list[Any], seconds: int = 20, *, cleanup: bool = False) -> str:
        child = start(name, argv, seconds)
        status = child.wait(timeout=seconds if cleanup else min(seconds, max(0.1, deadline - time.monotonic())))
        assert status == 0, f"{name} exited {status}; inspect its owned log"
        return (root / f"{name}.log").read_text()

    def stop(child: subprocess.Popen[bytes]) -> None:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=12)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)

    def interrupted(_signum: int, _frame: Any) -> None:
        raise TimeoutError("Interrupted or exceeded the 480-second profile test deadline")

    for sig in (signal.SIGINT, signal.SIGTERM, signal.SIGALRM):
        signal.signal(sig, interrupted)
    signal.alarm(480)
    write(plugin / "package.json", {"name": "existing-tool-hooks", "version": "0.0.1", "private": True,
                                   "type": "module", "openclaw": {"extensions": ["./index.mjs"]}})
    write(plugin / "openclaw.plugin.json", {"id": "existing-tool-hooks", "activation": {"onStartup": True},
          "configSchema": {"type": "object", "properties": {"trace": {"type": "string"}}}})
    (plugin / "index.mjs").write_text("""import { appendFileSync } from 'node:fs';
export default { id: 'existing-tool-hooks', register(api) {
  const record = row => appendFileSync(api.pluginConfig.trace, JSON.stringify({ pid: process.pid, ...row }) + '\\n');
  record({ phase: 'registered' });
  for (const phase of ['gateway_start', 'before_tool_call', 'after_tool_call']) {
    api.on(phase, event => { record({ phase, event }); }, { priority: 200 });
  }
}};
""")
    plugin_hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in plugin.iterdir()}
    (workspace / "AGENTS.md").write_text("Use only the exact harmless exec calls requested. Never read environment variables or files outside this workspace. Never retry a denied call.\n")
    base: dict[str, Any] = {
        "logging": {"file": str(root / "runtime.log"), "level": "info"},
        "models": {"mode": "merge", "providers": {"aw-token-plan": {
            "baseUrl": "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
            "api": "openai-completions", "models": [{"id": "qwen3.7-plus", "name": "qwen3.7-plus",
                "reasoning": False, "input": ["text"], "contextWindow": 131072, "maxTokens": 1024}]}}},
        "auth": {"profiles": {"aw-token-plan:profile-fixture": {"provider": "aw-token-plan", "mode": "api_key"}}},
        "agents": {"defaults": {"model": {"primary": "aw-token-plan/qwen3.7-plus"},
                                 "workspace": str(workspace), "skipBootstrap": True}},
        "tools": {"allow": ["exec"], "exec": {"mode": "full"}},
        "gateway": {"mode": "local", "bind": "loopback", "port": port,
                    "auth": {"mode": "token", "token": uuid.uuid4().hex}, "controlUi": {"enabled": False}},
        "skills": {"load": {"watch": False}}, "cron": {"enabled": False},
        "plugins": {"allow": ["existing-tool-hooks"], "entries": {"existing-tool-hooks": {
            "enabled": True, "config": {"trace": str(root / "existing-hooks.jsonl")}}}},
    }
    write(native, base)
    config = {"apiVersion": "aw/v1alpha1", "kind": "AWConfiguration", "metadata": {"name": "openclaw-profile"},
        "spec": {"daemon": {"startup": "on_demand", "endpoint": "auto", "state_dir": str(state)},
            "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
            "audit": {"enabled": True, "payload": "metadata_only"},
            "agents": {"openclaw": {"adapter": "openclaw", "argv": [str(node), str(cli), "gateway", "run",
                "--bind", "loopback", "--port", str(port), "--tailscale", "off"]}},
            "providers": {"policy": {"protocol": "aw-provider/v1alpha1", "transport": {"type": "stdio", "location": "agent",
                "argv": ["python3", str(repo / "src/aw/examples/providers/policy.py")]}, "timeout_ms": 5000,
                "max_output_bytes": 65536, "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]}}},
            "events": {event: {"enabled": True, "required": True, "steps": [{"id": operation, "provider": "policy",
                "operation": operation, "effects": effects, "on_error": failure}]} for event, operation, effects, failure in [
                    ("tool.before", "check", ["observe", "block"], "block"), ("tool.after", "record", ["observe"], "report")]}}}
    config_path = root / "aw.json"
    write(config_path, config)
    auth = [node, Path(__file__).with_name("profile_auth.mjs")]
    session = str(uuid.uuid4())
    first_transcript: list[tuple[Any, ...]] = []

    def transcript() -> list[tuple[Any, ...]]:
        database = profile / "agents/main/agent/openclaw-agent.sqlite"
        connection = sqlite3.connect(f"file:{database}?mode=ro", uri=True)
        try:
            return connection.execute("SELECT seq, event_json, event_zstd FROM transcript_events WHERE session_id = ? ORDER BY seq", (session,)).fetchall()
        finally:
            connection.close()

    try:
        run("auth-seed", [*auth, "seed", package, args.key_file])
        run("validate", [aw, "validate", "--config", config_path])
        for round_number in (1, 2):
            if round_number == 2:
                base["tools"]["exec"]["mode"] = "deny"
                write(native, base)
            baseline = native.read_bytes()
            write(root / f"native-round-{round_number}.json", base)
            run(f"auth-before-{round_number}", [*auth, "inspect", package, args.key_file])
            gateway = start(f"gateway-{round_number}", [aw, "run", "openclaw", "--config", config_path,
                "--native-config", native, "--native-state-dir", profile, "--state-dir", state, "--socket", endpoint], 200)
            ready_deadline = min(deadline, time.monotonic() + 45)
            receipt = None
            while time.monotonic() < ready_deadline:
                assert gateway.poll() is None, f"Gateway {round_number} exited before AW registration"
                files = list(state.glob("launch-*/registration.json"))
                if len(files) == 1:
                    receipt = json.loads(files[0].read_text())
                    break
                time.sleep(0.2)
            assert receipt and receipt["adapter"] == "openclaw" and receipt["hooks"] == 2, "Missing AW Gateway receipt"
            write(root / f"receipt-{round_number}.json", receipt)
            records.append({"name": f"native-gateway-{round_number}", "pid": receipt["pid"],
                "pgid": os.getpgid(receipt["pid"]), "command": config["spec"]["agents"]["openclaw"]["argv"],
                "cwd": str(repo), "ports": [port], "log": str(root / f"gateway-{round_number}.log"),
                "timeout_seconds": 200, "stop": f"kill -TERM -- -{os.getpgid(receipt['pid'])}"})
            save()
            status = json.loads(run(f"status-{round_number}", [aw, "status", "--socket", endpoint]))
            if daemon_pid is None:
                daemon_pid = status["pid"]
                records.append({"name": "on-demand-daemon", "pid": daemon_pid,
                    "pgid": os.getpgid(daemon_pid), "command": [str(aw), "serve", "--config", str(config_path),
                    "--socket", str(endpoint), "--idle-timeout", "300"], "cwd": str(repo), "ports": [],
                    "log": str(state / "daemon.log"), "timeout_seconds": 480,
                    "stop": f"{aw} stop --socket {endpoint}"})
                save()
            generated = json.loads(next(state.glob("launch-*/openclaw.json")).read_text())
            assert generated["plugins"]["entries"]["existing-tool-hooks"] == base["plugins"]["entries"]["existing-tool-hooks"]
            assert set(generated["plugins"]["allow"]) == {"existing-tool-hooks", "aw-native-hooks"}
            assert "apiKey" not in generated["models"]["providers"]["aw-token-plan"]
            if round_number == 2:
                assert first_transcript and transcript()[:len(first_transcript)] == first_transcript, "First session was lost at restart"
            prompt = (
                "This is a controlled hook test. Call exec exactly twice, sequentially: first run exactly "
                "printf 'AW_ALLOW_FIXTURE'. Then run exactly printf 'AW_DENY_FIXTURE'. These only print text. "
                "Report each tool's actual output or rejection. Do not retry, read files, inspect environment variables, "
                "combine calls, or use another tool."
                if round_number == 1 else
                "Continue the controlled test. Call exec exactly once with command: printf 'AW_NATIVE_DENY_FIXTURE'. "
                "Report the actual native denial, do not retry or use any other tool."
            )
            run(f"agent-{round_number}", [node, cli, "agent", "--session-id", session, "--model",
                "aw-token-plan/qwen3.7-plus", "--thinking", "off", "--timeout", "120", "--json", "--message", prompt], 140)
            stop(gateway)
            assert native.read_bytes() == baseline, "AW changed the original native configuration"
            assert not list(state.glob("launch-*")), "AW launch resources survived Gateway exit"
            run(f"auth-after-{round_number}", [*auth, "inspect", package, args.key_file])
            rows = transcript()
            assert rows, "Expected native SQLite session transcript was not persisted"
            if round_number == 1:
                first_transcript = rows
            result["rounds"].append({"round": round_number, "receipt": True, "native_config_unchanged": True,
                "credential_reused": True, "session_id": session, "transcript_events": len(rows),
                "session_database": "agents/main/agent/openclaw-agent.sqlite"})
        audit = [json.loads(row) for row in (state / "audit.jsonl").read_text().splitlines()]
        trace = [json.loads(row) for row in (root / "existing-hooks.jsonl").read_text().splitlines()]
        result["audit"] = [{"event": row["event"], "disposition": row["disposition"], "error": row["error"]} for row in audit]
        assert [(row["event"], row["disposition"]) for row in audit] == [
            ("tool.before", "observe"), ("tool.after", "observe"), ("tool.before", "block"),
            ("tool.after", "observe"), ("tool.before", "observe"), ("tool.after", "observe")], "Unexpected native event sequence"
        assert all(row["error"] is None for row in audit)
        after = [row["event"] for row in trace if row["phase"] == "after_tool_call"]
        assert len(after) == 3 and any("denied" in json.dumps(row).lower() for row in after), "Missing native denial after hook"
        assert any(row.get("result", {}).get("details", {}).get("status") == "blocked" for row in after), "Missing AW block after hook"
        assert all(hashlib.sha256((plugin / name).read_bytes()).hexdigest() == digest for name, digest in plugin_hashes.items())
        assert len({row["pid"] for row in trace if row["phase"] == "gateway_start"}) == 2
        assert transcript()[:len(first_transcript)] == first_transcript and len(transcript()) > len(first_transcript)
        result.update(status="passed", native_auth_profile_reused=True, model_key_in_environment=False,
                      plugin_directory_preserved=True, existing_plugin_coexisted=True, session_continued=True,
                      success_and_native_denial_after=True, aw_block_after_observed=True)
    except BaseException as error:
        result.update(status="failed", error=str(error))
        raise
    finally:
        signal.alarm(0)
        for child, record in reversed(children):
            stop(child)
            record.update(exit_code=child.returncode, pid_absent=not Path(f"/proc/{child.pid}").exists())
        if endpoint.exists():
            try:
                run("stop-daemon", [aw, "stop", "--socket", endpoint], 10, cleanup=True)
            except BaseException:
                if daemon_pid is not None and Path(f"/proc/{daemon_pid}").exists():
                    os.kill(daemon_pid, signal.SIGTERM)
        for _attempt in range(50):
            if not endpoint.exists() and (daemon_pid is None or not Path(f"/proc/{daemon_pid}").exists()):
                break
            time.sleep(0.1)
        with socket.socket() as probe:
            port_closed = probe.connect_ex(("127.0.0.1", port)) != 0
        stopped = all(child.poll() is not None for child, _record in children)
        daemon_absent = daemon_pid is None or not Path(f"/proc/{daemon_pid}").exists()
        registered_absent = all(not Path(f"/proc/{record['pid']}").exists() for record in records)
        cleanup: dict[str, Any] = {"owned_processes_stopped": stopped, "daemon_absent": daemon_absent,
            "socket_absent": not endpoint.exists(), "port": port, "port_closed": port_closed,
            "launch_directories": [str(p) for p in state.glob("launch-*")], "scrubbed_files": []}
        if stopped and daemon_absent and port_closed and registered_absent:
            replacement = b"REDACTED" + b"*" * max(0, len(key.encode()) - len(b"REDACTED"))
            for path in root.rglob("*"):
                if path.is_file() and key.encode() in path.read_bytes():
                    path.write_bytes(path.read_bytes().replace(key.encode(), replacement))
                    cleanup["scrubbed_files"].append(str(path.relative_to(root)))
        cleanup["credential_remaining"] = any(key.encode() in p.read_bytes() for p in root.rglob("*") if p.is_file())
        for child, record in children:
            record.update(exit_code=child.returncode, pid_absent=not Path(f"/proc/{child.pid}").exists())
        for record in records:
            record["pid_absent"] = not Path(f"/proc/{record['pid']}").exists()
        cleanup["registered_pids_absent"] = all(record["pid_absent"] for record in records)
        save()
        write(root / "cleanup.json", cleanup)
        write(root / "result.json", result)
        print(json.dumps({"status": result["status"], "evidence": str(root / "result.json"), "cleanup": cleanup}, indent=2))
        assert stopped and daemon_absent and port_closed and not endpoint.exists(), "Owned runtime cleanup incomplete"
        assert not cleanup["launch_directories"] and not cleanup["credential_remaining"] and cleanup["registered_pids_absent"]


if __name__ == "__main__":
    main()
