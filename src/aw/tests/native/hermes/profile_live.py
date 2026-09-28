#!/usr/bin/env python3
"""Bounded real Hermes CLI verification with an isolated native configuration."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--aw", type=Path, required=True)
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--provider", type=Path, required=True)
    parser.add_argument("--key-file", type=Path, required=True)
    args = parser.parse_args()

    def interrupted(_signal: int, _frame: object) -> None:
        raise KeyboardInterrupt("bounded Hermes probe interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    root = args.root.resolve()
    root.mkdir()
    work = root / "work"
    work.mkdir()
    host_home = root / "host-home"
    host_home.mkdir(mode=0o700)
    source_profile = root / "source-profile"
    source_profile.mkdir(mode=0o700)
    source_env = source_profile / ".env"
    source_env.write_text("AW_HERMES_PROFILE_SENTINEL=original\n")
    (source_profile / "auth.json").write_text("{}\n")
    (source_profile / "sessions").mkdir()
    (source_profile / "sessions/keep.txt").write_text("existing fixture session\n")
    trace = root / "trace.jsonl"
    shutil.copyfile(Path(__file__).with_name("profile_hook.py"), work / "native-hook.py")
    native = {"model": {"provider": "custom", "default": "qwen3.7-plus",
        "base_url": "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1",
        "key_env": "AW_HERMES_TEST_KEY", "api_mode": "chat_completions"},
        "terminal": {"backend": "local", "cwd": str(work)}, "agent": {"max_turns": 4},
        "memory": {"memory_enabled": False, "user_profile_enabled": False},
        "database": {"journal_mode": "delete"}, "hooks": {
            "pre_tool_call": [{"command": "/usr/bin/python3 ./native-hook.py native-before", "timeout": 5}],
            "post_tool_call": [{"command": "/usr/bin/python3 ./native-hook.py native-after", "timeout": 5}]}}
    native_path = source_profile / "config.yaml"
    native_path.write_text(json.dumps(native, indent=2))
    original_files = {str(path): hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in source_profile.rglob("*") if path.is_file()}
    prompt = root / "prompt.txt"
    prompt.write_text("Use terminal exactly twice, sequentially. First run printf 'AW_ALLOW_FIXTURE' > allowed.txt. "
        "Then run printf 'AW_DENY_FIXTURE' > denied.txt. Finally report whether each tool call succeeded. "
        "If any call is blocked, do not retry it or use another tool. Both filenames are inside the current fixture workspace.")
    # Preserve the venv executable path: resolving its symlink would bypass venv site packages.
    python = str(args.python.absolute())
    config = {"apiVersion": "aw/v1alpha1", "kind": "AWConfiguration", "metadata": {
        "name": "hermes-profile-common-policy"}, "spec": {
        "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
        "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
        "audit": {"enabled": True, "payload": "metadata_only"},
        "agents": {"hermes": {"adapter": "hermes", "argv": [python, "-m", "hermes_cli.main",
            "chat", "--oneshot", "--max-turns", "4", "--run-budget", "120", "--provider", "custom",
            "--model", "qwen3.7-plus", "--accept-hooks", "--ignore-rules", "--toolsets", "terminal",
            "--quiet", "--query-file", str(prompt)]}},
        "providers": {"policy": {"protocol": "aw-provider/v1alpha1", "transport": {
            "type": "stdio", "location": "agent", "argv": ["/usr/bin/python3", str(args.provider.resolve())]},
            "timeout_ms": 5000, "max_output_bytes": 65536,
            "config": {"blocked_substrings": ["AW_DENY_FIXTURE"]}}},
        "events": {
            "tool.before": {"enabled": True, "required": True, "steps": [{"id": "check", "provider": "policy",
                "operation": "check", "effects": ["observe", "block"], "on_error": "block"}]},
            "tool.after": {"enabled": True, "required": True, "steps": [{"id": "record", "provider": "policy",
                "operation": "record", "effects": ["observe"], "on_error": "report"}]}}}}
    config_path = root / "aw.json"
    config_path.write_text(json.dumps(config, indent=2))
    private = Path(tempfile.mkdtemp(prefix="aw-hermes-profile-"))
    socket = private / "aw.sock"
    state = root / "state"
    aw = str(args.aw.resolve())
    secret = args.key_file.read_bytes().strip()
    environment = {name: value for name, value in os.environ.items() if not name.startswith("HERMES_")}
    environment.update(HOME=str(host_home), HERMES_HOME=str(source_profile),
        PYTHONDONTWRITEBYTECODE="1", PYTHONPATH=str(args.source.resolve()),
        AW_HERMES_TEST_KEY=secret.decode(), AW_HERMES_TEST_TRACE=str(trace))
    ledger = {"processes": []}
    owned = []
    ledger_path = root / "processes.json"

    def start(command: list[str], name: str, seconds: int, env: dict) -> subprocess.Popen:
        log_path = root / (name + ".log")
        log = log_path.open("w")
        child = subprocess.Popen(command, cwd=work, env=env, stdout=log,
                                 stderr=subprocess.STDOUT, start_new_session=True)
        record = {"pid": child.pid, "process_group": child.pid, "command": command, "cwd": str(work),
                  "ports": [], "socket": str(socket), "log": str(log_path), "deadline_seconds": seconds,
                  "stop_command": f"kill -TERM -- -{child.pid}"}
        ledger["processes"].append(record)
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        owned.append((child, record, log))
        return child

    try:
        daemon = start([aw, "serve", "--config", str(config_path), "--socket", str(socket),
                        "--idle-timeout", "180"], "daemon", 180, dict(os.environ))
        deadline = time.monotonic() + 5
        while not socket.exists() and time.monotonic() < deadline:
            if daemon.poll() is not None:
                raise RuntimeError("AW daemon exited before readiness")
            time.sleep(0.05)
        if not socket.exists():
            raise TimeoutError("AW socket readiness timed out")
        child = start([aw, "run", "hermes", "--config", str(config_path), "--native-config", str(native_path),
            "--state-dir", str(state), "--socket", str(socket)], "agent", 150, environment)
        result_code = child.wait(timeout=150)
        rows = [json.loads(line) for line in trace.read_text().splitlines()]
        audit = [json.loads(line) for line in (private / "audit.jsonl").read_text().splitlines()]
        (root / "audit.jsonl").write_text("".join(json.dumps(row) + "\n" for row in audit))
        result = {"framework": "hermes", "entrypoint": "official CLI chat --oneshot", "exit_code": result_code,
            "native_hook_labels": [row["label"] for row in rows],
            "native_hook_cwds": sorted({row["cwd"] for row in rows}),
            "generated_homes": sorted({row["hermes_home"] for row in rows}),
            "source_profile_env_loaded": any(row["source_profile_sentinel_visible"] for row in rows),
            "allowed_marker": (work / "allowed.txt").read_text() if (work / "allowed.txt").exists() else None,
            "denied_marker_exists": (work / "denied.txt").exists(),
            "native_after_statuses": [row["payload"]["extra"].get("status") for row in rows if row["label"] == "native-after"],
            "audit_dispositions": [row["disposition"] for row in audit],
            "source_profile_files_unchanged": all(Path(path).is_file() and hashlib.sha256(
                Path(path).read_bytes()).hexdigest() == digest for path, digest in original_files.items())}
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        assert result_code == 0 and result["allowed_marker"] == "AW_ALLOW_FIXTURE", result
        assert not result["denied_marker_exists"] and result["source_profile_files_unchanged"], result
        assert result["native_hook_labels"] == ["native-before", "native-after", "native-before", "native-after"], result
        assert result["native_hook_cwds"] == [str(work)], result
        assert result["native_after_statuses"][-1] == "blocked", result
        assert result["audit_dispositions"] == ["observe", "observe", "block", "observe"], result
    finally:
        for child, record, log in reversed(owned):
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
            record.update(finished=True, exit_code=child.returncode)
            log.close()
        ledger_path.write_text(json.dumps(ledger, indent=2) + "\n")
        # Hermes may persist an env-supplied model key in its private auth pool.
        # Generated launch state is task-owned; the source profile contains no key.
        if state.exists():
            shutil.rmtree(state)
        shutil.rmtree(private)
        redacted = []
        for path in root.rglob("*"):
            if path.is_file() and secret and secret in path.read_bytes():
                path.write_bytes(path.read_bytes().replace(secret, b"[REDACTED]"))
                redacted.append(str(path.relative_to(root)))
        assert all(not secret or secret not in path.read_bytes() for path in root.rglob("*") if path.is_file())
        cleanup = {"private_runtime_removed": not private.exists(), "generated_state_removed": not state.exists(),
                   "redacted_files": redacted, "key_file_ownership": "parent; retained for parallel tests",
                   "owned_pids_absent": all(not Path(f"/proc/{child.pid}").exists() for child, _, _ in owned)}
        (root / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")


if __name__ == "__main__":
    main()
