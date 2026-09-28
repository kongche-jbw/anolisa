#!/usr/bin/env python3
"""Verify common Provider allow/block/observe effects through real Qoder calls."""

import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--aw", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--sec-core", type=Path, help="Installed V1 sec-core CLI with local regex audit")
    args = parser.parse_args()

    def interrupted(_signal: int, _frame: object) -> None:
        raise KeyboardInterrupt("bounded probe interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    root = args.root.resolve()
    root.mkdir()
    project = root / "project"
    project.mkdir()
    state = root / "state"
    state.mkdir(mode=0o700)
    socket_dir = args.config.parent / "structured"
    socket_dir.mkdir(mode=0o700)
    socket = socket_dir / "aw.sock"
    fixture = Path(__file__).with_name("hook_fixture.py").resolve()
    trace = root / "hooks.jsonl"
    config = {"apiVersion": "aw/v1alpha1", "kind": "AWConfiguration",
        "metadata": {"name": "qoder-common-provider"}, "spec": {
            "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
            "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
            "audit": {"enabled": True, "payload": "metadata_only"},
            "agents": {"qoder": {"adapter": "qoder", "argv": [str(args.binary.resolve())]}},
            "providers": {"policy": {"protocol": "aw-provider/v1alpha1", "transport": {
                "type": "stdio", "location": "agent", "argv": ["/usr/bin/python3",
                    str(args.provider.resolve())]}, "timeout_ms": 5000,
                "max_output_bytes": 1048576,
                "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]}}},
            "events": {
                "tool.before": {"enabled": True, "required": True, "steps": [{"id": "check",
                    "provider": "policy", "operation": "check", "effects": ["observe", "block"],
                    "on_error": "block"}]},
                "tool.after": {"enabled": True, "required": True, "steps": [{"id": "record",
                    "provider": "policy", "operation": "record", "effects": ["observe"],
                    "on_error": "report"}]}}}}
    denied_text = "AW_DENY_FIXTURE"
    if args.sec_core:
        scanner_data = root / "audit-scanner"
        scanner_data.mkdir(mode=0o700)
        provider = config["spec"]["providers"]["policy"]
        provider["config"] = {"sec_core": {"argv": [str(args.sec_core.resolve())],
            "timeout_ms": 3000, "block_verdicts": ["warn", "deny"]}}
        provider["transport"]["env"] = {"AGENT_SEC_DATA_DIR": str(scanner_data),
                                         "PYTHONDONTWRITEBYTECODE": "1"}
        provider["timeout_ms"] = 6000
        config["spec"]["execution"]["default_event_budget_ms"] = 6000
        # The regex sees command-like text, while the requested command only prints it.
        denied_text = "rm -rf /aw-never-executed"
    (root / "aw.json").write_text(json.dumps(config, indent=2))
    native = {"hooks": {name: [{"matcher": "Bash", "hooks": [{"type": "command",
        "command": "/usr/bin/python3", "args": [str(fixture), "--trace", str(trace),
        "--name", "native-" + event], "timeout": 5}]}] for name, event in (
            ("PreToolUse", "before"), ("PostToolUse", "after"))}}
    (root / "native.json").write_text(json.dumps(native))
    aw = str(args.aw.resolve())
    ledger = {"processes": []}
    ledger_path = root / "processes.json"

    def start(command: list[str], name: str, seconds: int) -> tuple:
        log = root / (name + ".log")
        output = log.open("w")
        child = subprocess.Popen(command, cwd=project, stdout=output,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        record = {"command": command, "cwd": str(project), "pid": child.pid,
            "process_group": child.pid, "ports": [], "log": str(log),
            "deadline_seconds": seconds, "stop_command": f"kill -TERM -- -{child.pid}"}
        ledger["processes"].append(record)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        return child, record, output

    daemon, daemon_record, daemon_log = start([aw, "serve", "--config", str(root / "aw.json"),
        "--socket", str(socket), "--idle-timeout", "120"], "daemon", 120)
    child = None
    try:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not socket.exists():
            if daemon.poll() is not None:
                raise RuntimeError("AW exited before readiness")
            time.sleep(0.05)
        if not socket.exists():
            raise TimeoutError("AW socket readiness timed out")
        prompt = ("Call Bash once with printf 'AW_ALLOW_FIXTURE'. After it completes, call Bash once "
            f"with printf '{denied_text}'. These are printf-only calls; do not execute the quoted text. "
            "Finally report the first output and whether the second "
            "was denied. Never retry or use additional tools.")
        command = [aw, "run", "qoder", "--config", str(root / "aw.json"),
            "--state-dir", str(state), "--socket", str(socket), "--native-config", str(root / "native.json"),
            "--", "--config-dir", str(args.config), "--strict-mcp-config", "--mcp-config",
            '{"mcpServers":{}}', "--no-session-persistence", "--max-model-request-retries", "0",
            "--max-output-tokens", "512", "--system-prompt",
            "Perform exactly two requested Bash calls, sequentially. Then finish. Never retry.",
            "--allowed-tools", "Bash", "--tools", "Bash", "-p", prompt]
        child, child_record, child_log = start(command, "qoder", 90)
        try:
            result_code = child.wait(timeout=90)
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
            child_record.update(finished=True, exit_code=child.returncode)
            child_log.close()
        events = [json.loads(line) for line in trace.read_text().splitlines()]
        starts = [entry for entry in events if entry["phase"] == "start"]
        before = [entry for entry in starts if entry["name"] == "native-before"]
        after = [entry for entry in starts if entry["name"] == "native-after"]
        audit = [json.loads(line) for line in (socket_dir / "audit.jsonl").read_text().splitlines()]
        (root / "audit.jsonl").write_text("".join(json.dumps(entry) + "\n" for entry in audit))
        result = {"exit_code": result_code, "native_before_calls": len(before),
            "native_after_calls": len(after), "allowed_stdout": [
                entry["payload"].get("tool_response", {}).get("stdout") for entry in after],
            "denied_after_seen": any(denied_text in entry["payload"].get(
                "tool_input", {}).get("command", "") for entry in after), "audit": audit}
        if args.sec_core:
            scans = [json.loads(line) for line in (
                root / "audit-scanner/security-events.jsonl").read_text().splitlines()]
            result["scanner"] = [{"request": entry["details"]["request"]["code"],
                "verdict": entry["details"]["result"]["verdict"],
                "engine_version": entry["details"]["result"]["engine_version"]} for entry in scans]
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        assert result_code == 0 and len(before) == 2 and len(after) == 1, result
        assert result["allowed_stdout"] == ["AW_ALLOW_FIXTURE"] and not result["denied_after_seen"], result
        assert any(entry.get("disposition") == "block" for entry in audit), result
        if args.sec_core:
            assert [entry["verdict"] for entry in result["scanner"]] == ["pass", "warn"], result
            assert [entry["request"] for entry in result["scanner"]] == [
                "printf 'AW_ALLOW_FIXTURE'", "printf 'rm -rf /aw-never-executed'"], result
    finally:
        if child and child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            child.wait(timeout=5)
        if daemon.poll() is None:
            subprocess.run([aw, "stop", "--socket", str(socket)], capture_output=True, timeout=5)
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(daemon.pid, signal.SIGTERM)
                daemon.wait(timeout=5)
        daemon_record.update(finished=True, exit_code=daemon.returncode)
        daemon_log.close()
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")


if __name__ == "__main__":
    main()
