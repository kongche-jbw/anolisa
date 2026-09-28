#!/usr/bin/env python3
"""Run installed Hermes/OpenClaw native boundaries against one common AW policy."""

import json
import os
import signal
import subprocess
import time
from pathlib import Path


repo = Path(__file__).resolve().parents[5]
root = repo / "target/provider-lab/bridges"
root.mkdir(parents=True, exist_ok=False, mode=0o700)
aw = repo / "src/aw/target/debug/aw"
socket = root / "aw.sock"
deadline = time.monotonic() + 330
records = []
children = []


def save():
    (root / "processes.json").write_text(json.dumps(records, indent=2) + "\n")


def start(name, argv, environment, seconds):
    log = root / f"{name}.log"
    record = {"command": list(map(str, argv)), "cwd": str(repo), "ports": [], "socket": str(socket),
              "log": str(log), "timeout_seconds": seconds, "pid": None}
    records.append(record)
    save()
    with log.open("wb") as output:
        process = subprocess.Popen(record["command"], cwd=repo, env=environment, stdin=subprocess.DEVNULL,
                                   stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
    record.update(pid=process.pid, pgid=process.pid, stop=f"kill -TERM -- -{process.pid}")
    children.append((process, record))
    save()
    return process


def wait(process, seconds):
    status = process.wait(timeout=min(seconds, max(0.1, deadline - time.monotonic())))
    assert status == 0, f"Owned process {process.pid} failed with {status}; inspect its recorded log"


def interrupted(signum, frame):
    raise KeyboardInterrupt()


signal.signal(signal.SIGINT, interrupted)
signal.signal(signal.SIGTERM, interrupted)
environment = dict(os.environ)
environment.update(AW_BIN=str(aw), AW_SOCKET=str(socket))
config = {
    "apiVersion": "aw/v1alpha1", "kind": "AWConfiguration", "metadata": {"name": "provider-bridges"},
    "spec": {
        "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
        "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
        "audit": {"enabled": True, "payload": "metadata_only"},
        "agents": {name: {"adapter": name, "argv": ["/bin/true"]} for name in ["qoder", "openclaw", "hermes", "qwenpaw"]},
        "providers": {"policy": {"protocol": "aw-provider/v1alpha1", "transport": {"type": "stdio", "location": "agent", "argv": ["python3", str(repo / "src/aw/examples/providers/policy.py")]}, "timeout_ms": 5000, "max_output_bytes": 65536, "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]}}},
        "events": {event: {"enabled": True, "required": True, "steps": [{"id": operation, "provider": "policy", "operation": operation, "effects": effects, "on_error": policy}]} for event, operation, effects, policy in [("tool.before", "check", ["observe", "block"], "block"), ("tool.after", "record", ["observe"], "report")]},
    },
}
(root / "aw.json").write_text(json.dumps(config, indent=2) + "\n")
evidence = {}
try:
    server = start("daemon", [aw, "serve", "--config", root / "aw.json", "--socket", socket, "--idle-timeout", "300"], environment, 330)
    ready = time.monotonic() + 5
    while not socket.exists():
        assert server.poll() is None, "AW daemon exited before readiness"
        assert time.monotonic() < ready, "AW daemon readiness timeout"
        time.sleep(0.02)
    scripts = Path(__file__).resolve().parent
    for framework in ["hermes", "openclaw"]:
        state = root / framework
        state.mkdir(mode=0o700)
        env = dict(environment, AW_BRIDGE_RESULT=str(state / "result.json"))
        if framework == "hermes":
            env.update(HERMES_HOME=str(state), PYTHONPATH=str(repo / "target/native-lab/hermes/source"))
            env.pop("HERMES_SAFE_MODE", None)
            argv = [repo / "target/native-lab/hermes/venv/bin/python", scripts / "hermes.py"]
        else:
            runtime = repo / "target/native-lab/openclaw/runtime/node_modules"
            native = state / "openclaw.json"
            native.write_text(json.dumps({"logging": {"file": str(state / "runtime.log"), "level": "warn"}}))
            env.update(OPENCLAW_PACKAGE_DIR=str(runtime / "openclaw"), OPENCLAW_HOME=str(state), OPENCLAW_STATE_DIR=str(state), OPENCLAW_CONFIG_PATH=str(native))
            argv = [runtime / "node/bin/node", scripts / "openclaw.mjs"]
        wait(start(framework, argv, env, 60), 60)
        evidence[framework] = json.loads((state / "result.json").read_text())
    audit = [json.loads(line) for line in (root / "audit.jsonl").read_text().splitlines()]
    for framework in ["hermes", "openclaw"]:
        rows = [row for row in audit if row["agent"] == framework]
        assert [row["disposition"] for row in rows] == ["observe", "block", "observe"], rows
        assert [row["event"] for row in rows] == ["tool.before", "tool.before", "tool.after"]
        assert all(row["error"] is None for row in rows), rows
        evidence[framework]["audit_calls"] = 3
finally:
    try:
        if socket.exists():
            wait(start("stop", [aw, "stop", "--socket", socket], environment, 5), 5)
    finally:
        # A stop RPC failure must not prevent cleanup of the recorded groups.
        for process, record in reversed(children):
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)
            record.update(exit_code=process.returncode, pid_absent=not Path(f"/proc/{process.pid}").exists())
        save()
    assert not socket.exists(), "owned AW socket survived teardown"
    assert all(record["pid_absent"] for record in records), records
evidence["cleanup"] = {"owned_pids_absent": True, "socket_absent": True, "ports": []}
(root / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
print(json.dumps(evidence, indent=2))
