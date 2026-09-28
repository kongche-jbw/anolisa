#!/usr/bin/env python3
"""Run one authorized, bounded model case through Qoder and the AW daemon."""

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
    parser.add_argument("--case", choices=("parallel", "ask"), required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    case = root / ("model-" + args.case)
    case.mkdir()
    fixture = Path(__file__).with_name("hook_fixture.py").resolve()
    trace = case / "hooks.jsonl"
    marker = root / "project" / (args.case + "-marker")
    transformed = root / "project" / (args.case + "-transformed-marker")
    ask_marker = root / "project" / "ask-denied-marker"
    token = "AW_NATIVE_POST_71c5f6"
    providers = {}
    before = []
    after = []

    def hook(name: str, event: str, response: dict, contains: str | None = None) -> None:
        response_path = case / (name + ".json")
        response_path.write_text(json.dumps(response))
        providers[name] = {"protocol": "native-hook/v1alpha1", "transport": {
            "type": "stdio", "location": "agent", "argv": ["/usr/bin/python3",
                str(fixture), "--trace", str(trace), "--name", name, "--delay", "0.4",
                "--response", str(response_path)]}, "timeout_ms": 5000,
            "max_output_bytes": 1048576, "config": {}}
        if contains:
            providers[name]["transport"]["argv"].extend(["--when-command-contains", contains])
        step = {"id": name, "provider": name, "native": {}}
        if args.case == "ask":
            step["native"]["sequential"] = True
        (before if event == "tool.before" else after).append(step)

    if args.case == "parallel":
        hook("before-one", "tool.before", {"hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "allow"}})
        hook("before-two", "tool.before", {})
        hook("after-one", "tool.after", {"hookSpecificOutput": {
            "hookEventName": "PostToolUse", "updatedToolOutput": token}})
        hook("after-two", "tool.after", {})
    else:
        hook("rewrite", "tool.before", {"hookSpecificOutput": {"hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "updatedInput": {"command": f"printf 'rewritten' > '{transformed}'; printf 'ORIGINAL_TOOL'"}}}, "native-original")
        hook("ask", "tool.before", {"hookSpecificOutput": {"hookEventName": "PreToolUse",
            "permissionDecision": "ask", "permissionDecisionReason": "AW native ask test requires user approval."}}, "ASK_SECOND")
        hook("after-rewrite", "tool.after", {"hookSpecificOutput": {
            "hookEventName": "PostToolUse", "updatedToolOutput": token}})
    config = {"apiVersion": "aw/v1alpha1", "kind": "AWConfiguration", "metadata": {
        "name": "qoder-native-" + args.case}, "spec": {
        "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
        "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
        "audit": {"enabled": True, "payload": "metadata_only"},
        "agents": {"qoder": {"adapter": "qoder", "argv": [str(root / "bin/qodercli")]}},
        "providers": providers, "events": {
            "tool.before": {"enabled": True, "required": True, "steps": before},
            "tool.after": {"enabled": True, "required": True, "steps": after}}}}
    config_path = case / "aw.json"
    config_path.write_text(json.dumps(config, indent=2))
    state = case / "state"
    state.mkdir(mode=0o700)
    socket = state / "aw.sock"
    aw = str(args.aw.resolve())
    # Keep the Unix-domain path below Linux's 108-byte sockaddr_un bound.
    if len(str(socket).encode()) >= 108:
        socket = args.config.parent / ("qoder-" + args.case + ".sock")
    ledger_path = root / "processes.json"
    ledger = json.loads(ledger_path.read_text())

    def start(command: list[str], name: str, deadline: int) -> tuple:
        log = case / (name + ".log")
        output = log.open("w")
        process = subprocess.Popen(command, cwd=root / "project", stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        record = {"command": command, "cwd": str(root / "project"), "pid": process.pid,
                  "process_group": process.pid, "ports": [], "log": str(log),
                  "deadline_seconds": deadline, "stop_command": f"kill -TERM -- -{process.pid}"}
        ledger["processes"].append(record)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        return process, record, output

    daemon, daemon_record, daemon_log = start([aw, "serve", "--config", str(config_path),
        "--socket", str(socket), "--idle-timeout", "120"], "daemon", 120)
    launch = None
    try:
        deadline = time.monotonic() + 5
        while not socket.exists() and time.monotonic() < deadline:
            if daemon.poll() is not None:
                raise RuntimeError("AW daemon exited before readiness")
            time.sleep(0.05)
        if not socket.exists():
            raise TimeoutError("AW daemon readiness timed out")
        prompt = (f"Call Bash exactly once with this command: printf 'native-original' > '{marker}'; "
                  "printf 'native-original'. Then finish with the tool result. If the tool is denied, "
                  "finish with DENIED. Do not retry or call other tools.")
        system = "You perform exactly one requested Bash call and then finish. Never retry tool calls."
        if args.case == "ask":
            prompt = (f"Call Bash with command: printf 'native-original' > '{marker}'; printf 'native-original'. "
                      f"Then call Bash with command: printf 'ASK_SECOND' > '{ask_marker}'. "
                      "These are the only two tool calls. Finally report the first tool output exactly, "
                      "then whether the second was denied. Never retry a denied call.")
            system = "Perform exactly two requested Bash calls, sequentially. Then finish. Never retry tool calls."
        command = [aw, "run", "qoder", "--config", str(config_path), "--state-dir", str(state),
            "--socket", str(socket), "--", "--config-dir", str(args.config),
            "--strict-mcp-config", "--mcp-config", '{"mcpServers":{}}',
            "--no-session-persistence", "--max-model-request-retries", "0",
            "--max-output-tokens", "512", "--system-prompt",
            system,
            "--allowed-tools", "Bash", "--tools", "Bash", "-p", prompt]
        launch, launch_record, launch_log = start(command, "qoder", 90)
        try:
            return_code = launch.wait(timeout=90)
        except subprocess.TimeoutExpired:
            return_code = 124
        finally:
            if launch.poll() is None:
                os.killpg(launch.pid, signal.SIGTERM)
                try:
                    launch.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(launch.pid, signal.SIGKILL)
                    launch.wait(timeout=5)
            launch_record.update(finished=True, exit_code=return_code)
            launch_log.close()
        events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
        starts = [event for event in events if event["phase"] == "start"]
        result = {"case": args.case, "exit_code": return_code,
                  "marker_exists": marker.exists(), "transformed_marker_exists": transformed.exists(),
                  "ask_marker_exists": ask_marker.exists(),
                  "hook_starts": [event["name"] for event in starts],
                  "native_calls": len({event["payload"].get("tool_use_id") for event in starts}),
                  "output_rewrite_seen": token in (case / "qoder.log").read_text(),
                  "phases": [[event["name"], event["phase"]] for event in events]}
        if args.case == "ask":
            asks = [event for event in starts if event["name"] == "ask"]
            result["sequential_input_rewrite_seen"] = any(str(transformed) in
                event["payload"].get("tool_input", {}).get("command", "") for event in asks)
            result["ask_headless_denied"] = (any("ASK_SECOND" in
                event["payload"].get("tool_input", {}).get("command", "") for event in asks)
                and not ask_marker.exists())
        (case / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        if args.case == "parallel":
            assert marker.exists() and result["output_rewrite_seen"], result
        else:
            assert (transformed.exists() and result["sequential_input_rewrite_seen"]
                    and result["ask_headless_denied"] and result["output_rewrite_seen"]), result
    finally:
        if launch and launch.poll() is None:
            os.killpg(launch.pid, signal.SIGTERM)
            launch.wait(timeout=5)
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
