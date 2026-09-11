#!/usr/bin/env python3
"""Explicit native-hook smoke; frozen model responses are not a live-model eval.

Requires a prebuilt aw-hook-cli and the repository SecCore dependencies installed
from its frozen lock in Python 3.11.6. Never changes HOME or user hook settings.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
AW = ROOT / "src/aw"
TEXT = "api_key=sk-abcdefghijklmnopqrstuvwxyz123456\n"
COMMAND = "printf 'api_key=sk-abcdefghijklmnopqrstuvwxyz123456\\n'"
CAPTURE = """
import os, sys
from pathlib import Path
data = sys.stdin.buffer.read(1048577)
if len(data) > 1048576:
    raise SystemExit(1)
Path(sys.argv[1]).write_bytes(data)
with Path(sys.argv[1]).open("rb") as source:
    os.dup2(source.fileno(), 0)
os.execv(sys.argv[2], sys.argv[2:])
"""
NATIVE = """
import os
from pathlib import Path
from unittest.mock import patch
from agent_sec_cli.cli import main
with patch('agent_sec_cli.pii_checker.custom_rules.default_custom_rules_path',
           return_value=Path(os.environ['AW_SMOKE_RULES'])):
    main()
"""


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ticks(pid: int) -> int:
    return int(Path(f"/proc/{pid}/stat").read_bytes().rsplit(b")", 1)[1].split()[19])


class FixtureServer(HTTPServer):
    """Bounded scripted model responses with evidence of the actual second input."""

    def __init__(self, root: Path, command: str) -> None:
        super().__init__(("127.0.0.1", 0), FixtureHandler)
        self.root = root
        self.command = command
        self.requests = []
        self.failure = None
        self.timeout = 1


class FixtureHandler(BaseHTTPRequestHandler):
    def setup(self) -> None:
        super().setup()
        self.connection.settimeout(5)

    def log_message(self, *_args: Any) -> None:
        pass

    def do_POST(self) -> None:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            self.send_error(400)
            return
        if (
            self.path != "/v1/responses"
            or not 0 < length <= 2 * 1024 * 1024
            or len(self.server.requests) >= 2
        ):
            self.server.failure = "Unexpected model request, size, or request count"
            self.send_error(400)
            return
        try:
            payload = json.loads(self.rfile.read(length))
        except (ValueError, OSError):
            self.server.failure = "Invalid model request"
            self.send_error(400)
            return
        self.server.requests.append(payload)
        number = len(self.server.requests)
        write_json(self.server.root / f"model-request-{number}.json", payload)
        response_id = f"aw-r{number}"
        if number == 1:
            item = {
                "type": "function_call",
                "call_id": "aw-call-1",
                "name": "exec_command",
                "arguments": json.dumps(
                    {
                        "cmd": self.server.command,
                        "login": False,
                        "yield_time_ms": 1000,
                        "max_output_tokens": 1000,
                    }
                ),
            }
        else:
            item = {
                "type": "message",
                "role": "assistant",
                "id": "aw-msg-1",
                "content": [{"type": "output_text", "text": "AW hook smoke completed"}],
            }
        frames = [
            {"type": "response.created", "response": {"id": response_id}},
            {"type": "response.output_item.done", "item": item},
            {
                "type": "response.completed",
                "response": {
                    "id": response_id,
                    "usage": {
                        "input_tokens": 0,
                        "input_tokens_details": None,
                        "output_tokens": 0,
                        "output_tokens_details": None,
                        "total_tokens": 0,
                    },
                },
            },
        ]
        data = "".join(
            f"event: {frame['type']}\ndata: {json.dumps(frame)}\n\n" for frame in frames
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


def settings(root: Path, python: Path, agent_pid: int, host: str) -> dict:
    """Bind the owned process and selected repository scanner files explicitly."""
    source = ROOT / "src/agent-sec-core/agent-sec-cli/src"
    pins = [
        source / "agent_sec_cli" / path
        for path in (
            "cli.py",
            "pii_checker/scanner.py",
            "pii_checker/custom_rules.py",
            "pii_checker/detectors/custom.py",
            "pii_checker/detectors/regex.py",
            "security_middleware/backends/pii_scan.py",
        )
    ]
    runtime = {
        "runtime_id": "smoke-runtime",
        "generation": 1,
        "binding_revision": 1,
        "environment_id": "smoke-environment",
        "process_ref": f"pid:{agent_pid}@{ticks(agent_pid)}",
        "observation_source": "owned_child",
        "owner_id": "native-smoke",
        "state": "running",
        "sequence": 1,
    }
    return {
        "runtime": runtime,
        "scope": {
            "environment_id": runtime["environment_id"],
            "execution_context_id": "smoke-context",
            "actor_id": "smoke-agent",
            "runtime_id": runtime["runtime_id"],
            "runtime_generation": 1,
            "binding_revision": 1,
        },
        "agent_pid": agent_pid,
        "agent_start_ticks": ticks(agent_pid),
        "qoder_single_turn_id": "smoke-turn" if host == "qoder" else None,
        "journal": str(root / "journal"),
        "include_low_confidence": False,
        "provider": {
            "provider_id": "sec-core",
            "provider_version": "native-smoke-0.12.0",
            "program": str(python),
            "program_sha256": digest(python),
            "cwd": str(root),
            "args": ["-P", "-c", NATIVE],
            "environment": {
                **{key: os.environ[key] for key in ("HOME",) if key in os.environ},
                "PYTHONPATH": str(source),
                "PYTHONDONTWRITEBYTECODE": "1",
                "AGENT_SEC_DATA_DIR": str(root / "native-data"),
                "AGENT_SEC_TELEMETRY_LOG_PATH": str(root / "telemetry.jsonl"),
                "AW_SMOKE_RULES": str(root / "absent-rules.yaml"),
            },
            "pins": [{"path": str(path), "state": {"sha256": digest(path)}} for path in pins]
            + [{"path": str(root / "absent-rules.yaml"), "state": "absent"}],
            "limits": {
                "timeout_ms": 15000,
                "input_bytes": 1048576,
                "output_bytes": 1048576,
                "stderr_bytes": 65536,
            },
        },
    }


def validate(root: Path, source_text: str) -> dict:
    """Require one settled Core call and an independently written native audit."""
    journals = list((root / "journal").glob("*.jsonl"))
    if len(journals) != 1:
        raise RuntimeError("expected exactly one actual hook journal")
    envelopes = [json.loads(line) for line in journals[0].read_text().splitlines()]
    previous = None
    if not envelopes:
        raise RuntimeError("empty Core journal")
    for sequence, envelope in enumerate(envelopes):
        actual = envelope.pop("digest")
        encoded = json.dumps(
            envelope, sort_keys=True, ensure_ascii=False, separators=(",", ":")
        ).encode()
        if (
            envelope["sequence"] != sequence
            or envelope["previous_digest"] != previous
            or actual != hashlib.sha256(encoded).hexdigest()
        ):
            raise RuntimeError("Core journal chain is invalid")
        previous = actual
    records = [entry["record"] for entry in envelopes]
    calls = [record for record in records if record.get("kind") == "invocation_settled"]
    if len(calls) != 1 or calls[0]["receipt"]["disposition"] != "produced":
        raise RuntimeError("real SecCore did not produce exactly one successful receipt")
    if (
        records[-1].get("kind") != "execution_settled"
        or records[-1]["execution"]["decision"] != "proceed"
    ):
        raise RuntimeError("Core execution did not settle successfully")
    expected_digest = hashlib.sha256(source_text.encode()).hexdigest()
    if records[0]["plan"]["source_digest"] != expected_digest:
        raise RuntimeError("Core plan source differs from actual fixture output")
    audit_path = root / "native-data/security-events.jsonl"
    audit = [json.loads(line) for line in audit_path.read_text().splitlines()]
    if len(audit) != 1 or audit[0]["category"] != "pii_scan":
        raise RuntimeError("missing or duplicate real SecCore middleware audit")
    details = audit[0]["details"]
    if (
        details["request"]["text_sha256"] != expected_digest
        or details["result"]["verdict"] != "deny"
    ):
        raise RuntimeError("real native audit does not bind the expected sensitive input")
    if details["result"]["summary"]["bytes_scanned"] != len(source_text.encode()):
        raise RuntimeError("native scanner byte coverage mismatch")

    def no_raw(value: Any) -> None:
        if isinstance(value, dict):
            for key, child in value.items():
                if key in {"raw_evidence", "redacted_text", "text"}:
                    raise RuntimeError("forbidden raw field in journal or audit")
                no_raw(child)
        elif isinstance(value, list):
            for child in value:
                no_raw(child)
        elif isinstance(value, str) and TEXT.rstrip("\n") in value:
            raise RuntimeError("raw fixture input leaked into journal or audit")

    no_raw([records, audit])
    return {
        "native_audit": 1,
        "core_calls": 1,
        "inspection": "sensitive",
        "source_digest": expected_digest,
        "enforcement": "not_attempted",
        "adoption": "not_observed",
    }


def wait_owned(process: subprocess.Popen, seconds: int) -> int:
    """Keep the leader unreaped until its whole process group is stopped."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        status = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if status is not None:
            return status.si_status if status.si_code == os.CLD_EXITED else -status.si_status
        time.sleep(0.05)
    raise RuntimeError("native process exceeded its smoke deadline")


def process_snapshot() -> dict:
    """Read Linux identities without exposing process commands or environments."""
    result = {}
    for path in Path("/proc").glob("[0-9]*/stat"):
        try:
            fields = path.read_bytes().rsplit(b")", 1)[1].split()
            result[int(path.parent.name)] = (int(fields[19]), int(fields[1]), fields[0])
        except (FileNotFoundError, ProcessLookupError):
            continue
    return result


def stop_owned(process: subprocess.Popen) -> None:
    """Allow hook cancellation before killing only identified owned descendants."""
    owned = {process.pid: ticks(process.pid)}

    def live() -> dict:
        snapshot = process_snapshot()
        for _ in range(len(snapshot)):
            found = {
                pid: item[0]
                for pid, item in snapshot.items()
                if item[1] in owned
                and item[1] in snapshot
                and snapshot[item[1]][0] == owned[item[1]]
                and pid not in owned
            }
            if not found:
                break
            owned.update(found)
        return {
            pid: start
            for pid, start in owned.items()
            if pid in snapshot and snapshot[pid][0] == start and snapshot[pid][2] != b"Z"
        }

    live()
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    deadline = time.monotonic() + 5
    while live() and time.monotonic() < deadline:
        time.sleep(0.05)
    for pid in live():
        try:
            os.kill(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    deadline = time.monotonic() + 2
    while live() and time.monotonic() < deadline:
        time.sleep(0.05)
    remaining = list(live())
    process.wait(timeout=5)
    if remaining:
        raise RuntimeError(f"owned process cleanup failed; remaining PIDs: {remaining}")


def captured_source(root: Path) -> str:
    payload = json.loads((root / "hook-input.json").read_bytes())
    response = payload["tool_response"]
    text = response["stdout"] if isinstance(response, dict) else response
    if not isinstance(text, str) or text not in (TEXT, TEXT.removesuffix("\n")):
        raise RuntimeError(f"actual hook input differs from the fixed tool fixture: {text!r}")
    return text


def run(args: argparse.Namespace, root: Path) -> dict:
    work = root / "work"
    work.mkdir()
    config = root / "settings.json"
    hook = shlex.join(
        [
            sys.executable,
            "-c",
            CAPTURE,
            str(root / "hook-input.json"),
            str(args.hook_bin),
            args.host,
            str(config),
        ]
    )
    if args.mode == "payload":
        write_json(config, settings(root, args.provider_python, os.getpid(), args.host))
        payload = {
            "hook_event_name": "PostToolUse",
            "session_id": "smoke-session",
            "tool_use_id": "smoke-call",
            "turn_id": "smoke-turn",
            "tool_name": "Bash",
            "tool_input": {"command": COMMAND},
            "tool_response": TEXT,
        }
        write_json(root / "hook-input.json", payload)
        command = [str(args.hook_bin), args.host, str(config)]
    elif args.host == "codex":
        command = []  # Filled after the bounded localhost fixture starts.
    else:
        native_settings = root / "qoder-settings.json"
        write_json(
            native_settings,
            {
                "hooks": {
                    "PostToolUse": [
                        {
                            "matcher": "^Bash$",
                            "hooks": [{"type": "command", "command": hook, "timeout": 30}],
                        }
                    ]
                }
            },
        )
        command = [
            shutil.which("qodercli") or "qodercli",
            "--config-dir",
            str(root / "qoder-config"),
            "--setting-sources",
            "project",
            "--settings",
            str(native_settings),
            "--no-session-persistence",
            "--strict-mcp-config",
            "--mcp-config",
            '{"mcpServers":{}}',
            "--tools",
            "Bash",
            "--allowed-tools",
            "Bash",
            "--max-model-request-retries",
            "0",
            "-p",
            f"Run only this synthetic read-only fixture: {COMMAND}. Then finish.",
        ]
    server = None
    thread = None
    process = None
    environment = {
        key: value
        for key, value in os.environ.items()
        if key in {"PATH", "LANG", "LC_ALL", "TZ", "HOME", "CODEX_HOME"}
    }
    try:
        if args.mode == "agent" and args.host == "codex":
            server = FixtureServer(root, COMMAND)
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.1})
            thread.start()
            overrides = {
                "model": '"gpt-5.5"',
                "model_provider": '"aw_fixture"',
                "model_providers.aw_fixture": '{name="AW fixture",base_url="http://127.0.0.1:'
                + str(server.server_port)
                + '/v1",wire_api="responses",requires_openai_auth=false,'
                "supports_websockets=false,request_max_retries=0,stream_max_retries=0,stream_idle_timeout_ms=10000}",
                "approval_policy": '"never"',
                "features.unified_exec": "true",
                "features.hooks": "true",
                "features.apps": "false",
                "features.plugins": "false",
                "mcp_servers": "{}",
                "hooks.PostToolUse": '[{matcher="^Bash$",hooks=[{type="command",command='
                + json.dumps(hook)
                + ",timeout=30}]}]",
                "log_dir": json.dumps(str(root / "codex-log")),
                "sqlite_home": json.dumps(str(root / "codex-state")),
            }
            command = [
                shutil.which("codex") or "codex",
                "exec",
                "--ephemeral",
                "--ignore-user-config",
                "--ignore-rules",
                "--skip-git-repo-check",
                "--dangerously-bypass-hook-trust",
                "--sandbox",
                args.codex_sandbox,
                "--json",
                "-C",
                str(work),
            ]
            for key, value in overrides.items():
                command += ["-c", f"{key}={value}"]
            command.append("Execute the one scripted read-only printf fixture and finish.")
        print(
            json.dumps(
                {
                    "command": command,
                    "cwd": str(work),
                    "output_dir": str(root),
                    "timeout_seconds": 90,
                    "server_port": server.server_port if server else None,
                    "log_paths": [str(root / "stdout.log"), str(root / "stderr.log")],
                }
            ),
            flush=True,
        )
        with (root / "stdout.log").open("wb") as stdout, (root / "stderr.log").open("wb") as stderr:
            gated = (
                command
                if args.mode == "payload"
                else ["/bin/sh", "-c", 'read -r gate; exec "$@"', "aw-smoke", *command]
            )
            process = subprocess.Popen(
                gated,
                cwd=work,
                env=environment,
                stdin=subprocess.PIPE,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
            print(
                json.dumps(
                    {
                        "pid": process.pid,
                        "process_group": process.pid,
                        "stop": f"kill -TERM -- -{process.pid}",
                    }
                ),
                flush=True,
            )
            if args.mode == "agent":
                write_json(config, settings(root, args.provider_python, process.pid, args.host))
            process.stdin.write(json.dumps(payload).encode() if args.mode == "payload" else b"go\n")
            process.stdin.close()
            result = wait_owned(process, 90)
        if result != 0:
            raise RuntimeError(
                f"native {args.host} {args.mode} failed with exit {result}; hook not verified"
            )
        source_text = captured_source(root)
        if server:
            if server.failure or len(server.requests) != 2:
                raise RuntimeError("actual Codex did not complete the two-request tool exchange")
            outputs = [
                entry
                for entry in server.requests[1].get("input", [])
                if entry.get("type") == "function_call_output"
                and entry.get("call_id") == "aw-call-1"
            ]
            if len(outputs) != 1 or not str(outputs[0].get("output", "")).endswith(source_text):
                raise RuntimeError(
                    "actual tool result was not preserved in the second model request"
                )
        proof = validate(root, source_text)
        return {
            "status": "passed",
            "mode": args.mode,
            "host": args.host,
            "real_agent_hook": args.mode == "agent",
            "native_trimmed_final_newline": source_text != TEXT,
            "model": (
                "not used"
                if args.mode == "payload"
                else ("scripted localhost" if server else "Qoder configured model")
            ),
            "codex_sandbox": args.codex_sandbox if server else None,
            "sandbox_verified": False,
            **proof,
        }
    finally:
        cleanup(process, server, thread)


def cleanup(process: Any, server: Any, thread: Any) -> None:
    """Close the local server even if process cleanup reports a failure."""
    try:
        if process is not None:
            stop_owned(process)
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
            if thread.is_alive():
                raise RuntimeError("fixture server did not stop")


def self_test() -> int:
    """Exercise acceptance failures without agents, credentials, or SecCore."""

    class AcceptanceTests(unittest.TestCase):
        def setUp(self) -> None:
            (AW / "target").mkdir(exist_ok=True)
            self.temporary = tempfile.TemporaryDirectory(
                prefix="native-smoke-test-", dir=AW / "target"
            )
            self.addCleanup(self.temporary.cleanup)
            self.root = Path(self.temporary.name)
            (self.root / "journal").mkdir()
            (self.root / "native-data").mkdir()
            expected = hashlib.sha256(TEXT.encode()).hexdigest()
            records = [
                {"plan": {"source_digest": expected}},
                {"kind": "invocation_settled", "receipt": {"disposition": "produced"}},
                {"kind": "execution_settled", "execution": {"decision": "proceed"}},
            ]
            previous = None
            envelopes = []
            for sequence, record in enumerate(records):
                envelope = {"sequence": sequence, "previous_digest": previous, "record": record}
                encoded = json.dumps(
                    envelope, sort_keys=True, ensure_ascii=False, separators=(",", ":")
                ).encode()
                previous = hashlib.sha256(encoded).hexdigest()
                envelopes.append({**envelope, "digest": previous})
            self.journal = self.root / "journal/event.jsonl"
            self.journal.write_text("".join(json.dumps(item) + "\n" for item in envelopes))
            self.audit = self.root / "native-data/security-events.jsonl"
            self.audit_record = {
                "category": "pii_scan",
                "details": {
                    "request": {"text_sha256": expected},
                    "result": {"verdict": "deny", "summary": {"bytes_scanned": len(TEXT.encode())}},
                },
            }
            self.audit.write_text(json.dumps(self.audit_record) + "\n")

        def test_chain_integrity(self) -> None:
            self.assertEqual(validate(self.root, TEXT)["native_audit"], 1)
            original = self.journal.read_text()
            self.journal.write_text(original.replace('"proceed"', '"preserve"'))
            with self.assertRaisesRegex(RuntimeError, "chain"):
                validate(self.root, TEXT)

        def test_native_evidence_is_required_and_bound(self) -> None:
            self.audit.unlink()
            with self.assertRaises(OSError):
                validate(self.root, TEXT)
            for mutation in ("digest", "bytes", "raw"):
                with self.subTest(mutation=mutation):
                    record = json.loads(json.dumps(self.audit_record))
                    if mutation == "digest":
                        record["details"]["request"]["text_sha256"] = "0" * 64
                    elif mutation == "bytes":
                        record["details"]["result"]["summary"]["bytes_scanned"] -= 1
                    else:
                        record["details"]["raw_evidence"] = TEXT
                    self.audit.write_text(json.dumps(record) + "\n")
                    with self.assertRaises(RuntimeError):
                        validate(self.root, TEXT)

        def test_server_cleanup_survives_process_cleanup_failure(self) -> None:
            server, thread = Mock(), Mock()
            thread.is_alive.return_value = False
            with patch(
                __name__ + ".stop_owned", side_effect=RuntimeError("owned process cleanup failed")
            ):
                with self.assertRaisesRegex(RuntimeError, "owned process cleanup failed"):
                    cleanup(Mock(), server, thread)
            server.shutdown.assert_called_once()
            server.server_close.assert_called_once()
            thread.join.assert_called_once_with(timeout=5)

    suite = unittest.defaultTestLoader.loadTestsFromTestCase(AcceptanceTests)
    if suite.countTestCases() != 3:
        raise RuntimeError("native smoke acceptance self-tests are missing")
    return 0 if unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful() else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument(
        "--codex-sandbox",
        choices=("read-only", "danger-full-access"),
        default="read-only",
        help="Explicit native Codex test profile; no fallback or sandbox verification",
    )
    parser.add_argument("--host", choices=("qoder", "codex"))
    parser.add_argument("--mode", choices=("payload", "agent"))
    parser.add_argument("--provider-python", type=Path)
    parser.add_argument("--hook-bin", type=Path, default=AW / "target/debug/aw-hook-cli")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if not args.host or not args.mode or not args.provider_python:
        parser.error("--host, --mode and --provider-python are required for a native run")
    if any(
        not path.is_absolute() or not path.is_file()
        for path in (args.provider_python, args.hook_bin)
    ):
        parser.error("provider Python and hook binary must exist at absolute paths")

    def interrupted(_signal: int, _frame: Any) -> None:
        raise KeyboardInterrupt("smoke interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix="native-smoke-", dir=AW / "target") as temporary:
        root = Path(temporary)
        try:
            result = run(args, root)
        except (OSError, RuntimeError, ValueError, KeyError, KeyboardInterrupt) as error:
            result = {
                "status": "failed",
                "mode": args.mode,
                "host": args.host,
                "reason": str(error),
            }
            # Logs contain only this synthetic fixture; diagnostics stay in tool output.
            for name in ("stdout.log", "stderr.log"):
                if (root / name).exists():
                    print(
                        f"{name}: {(root / name).read_text(errors='replace')[-6000:]}",
                        file=sys.stderr,
                    )
        print(json.dumps(result, indent=2), flush=True)
    print(json.dumps({"temporary_directory_removed": not root.exists()}), flush=True)
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
