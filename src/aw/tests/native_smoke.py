#!/usr/bin/env python3
"""Exercise native CLI hooks with isolated or explicitly reused Qoder login.

Codex uses a scripted localhost Responses server, a real CLI tool execution,
AW Core, and the configured real SecCore Provider. Qoder probes its isolated
login boundary by default; --qoder-existing-login reuses the current login without copying it.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
import shlex
import uuid
from typing import Any

FIXTURES = {
    "sensitive": (
        "api_key=sk-abcdefghijklmnopqrstuvwxyz123456\n",
        "printf 'api_key=sk-abcdefghijklmnopqrstuvwxyz123456\\n'",
    ),
    "clean": (
        "version=1\nmessage=synthetic-clean\n",
        "printf 'version=1\\nmessage=synthetic-clean\\n'",
    ),
}


def fixture(case: str) -> tuple[str, str]:
    return FIXTURES["clean" if case == "clean" else "sensitive"]


def write_json(path: Path, value: Any) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def start_ticks(pid: int) -> int:
    return int(Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19])


def make_settings(
    args: argparse.Namespace, process: subprocess.Popen, root: Path
) -> dict[str, Any]:
    ticks = start_ticks(process.pid)
    runtime = {
        "runtime_id": f"native-smoke-{uuid.uuid4()}",
        "generation": 1,
        "binding_revision": 1,
        "environment_id": "native-smoke-environment",
        "process_ref": f"linux-pid-{process.pid}-start-{ticks}",
        "observation_source": "owned_child",
        "owner_id": "native-smoke-launcher",
        "state": "running",
        "sequence": 1,
    }
    runner = "from agent_sec_cli.aw_provider.runner import run_provider; import sys; run_provider(sys.stdin, sys.stdout)"
    if args.case == "provider-failure":
        runner = "raise SystemExit(23)"
    return {
        "runtime": runtime,
        "scope": {
            "environment_id": runtime["environment_id"],
            "execution_context_id": f"context-{uuid.uuid4()}",
            "actor_id": "native-smoke-agent",
            "runtime_id": runtime["runtime_id"],
            "runtime_generation": 1,
            "binding_revision": 1,
        },
        "agent_pid": process.pid,
        "agent_start_ticks": ticks,
        "qoder_single_turn_id": "launcher-owned-single-turn" if args.host == "qoder" else None,
        "journal": str(root / "journal"),
        "evidence": str(root / "evidence"),
        "provider": {
            "provider_id": "sec-core",
            "provider_version": args.provider_version,
            "program": str(args.provider_python),
            "args": ["-P", "-c", runner],
            "environment": {
                "PYTHONPATH": str(args.provider_source),
                "PYTHONDONTWRITEBYTECODE": "1",
            },
        },
    }


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


def isolated_environment(home: Path) -> dict[str, str]:
    # Explicit allowlist: no inherited model tokens, proxy credentials, or agent settings.
    env = {key: os.environ[key] for key in ("PATH", "LANG", "LC_ALL", "TZ") if key in os.environ}
    env.update(
        {
            "HOME": str(home),
            "CODEX_HOME": str(home / ".codex"),
            "XDG_CONFIG_HOME": str(home / ".config"),
            "XDG_CACHE_HOME": str(home / ".cache"),
            "TMPDIR": str(home / "tmp"),
            "PYTHONDONTWRITEBYTECODE": "1",
        }
    )
    return env


def qoder_environment(temporary: Path) -> dict[str, str]:
    """Let Qoder access its normal login while keeping test caches separate."""
    env = isolated_environment(temporary)
    for key in ("HOME", "XDG_CONFIG_HOME"):
        if key in os.environ:
            env[key] = os.environ[key]
        else:
            env.pop(key, None)
    return env


def validate_codex(root: Path, server: FixtureServer, returncode: int, case: str) -> dict[str, Any]:
    expected_text, _ = fixture(case)
    if returncode != 0 or server.failure or len(server.requests) != 2:
        raise RuntimeError(
            "Native Codex process or bounded model exchange failed; inspect CLI logs"
        )
    tool_outputs = [
        entry
        for entry in server.requests[1].get("input", [])
        if entry.get("type") == "function_call_output" and entry.get("call_id") == "aw-call-1"
    ]
    if len(tool_outputs) != 1 or expected_text.strip() not in str(
        tool_outputs[0].get("output", "")
    ):
        raise RuntimeError("Second model request did not preserve the actual synthetic tool output")
    result = validate_inspection(root, case)
    result.update(model_requests=2, original_tool_output_in_second_request=True)
    return result


def validate_inspection(root: Path, case: str, native_text: str | None = None) -> dict[str, Any]:
    expected_text, _ = fixture(case)
    if native_text is not None:
        if native_text != expected_text.removesuffix("\n"):
            raise RuntimeError("Unexpected Qoder native stdout")
        expected_text = native_text
    paths = list((root / "evidence").glob("*.json"))
    if len(paths) != 1:
        raise RuntimeError("Expected exactly one native AW evidence record")
    evidence = json.loads(paths[0].read_text())
    if evidence["source_digest"] != hashlib.sha256(expected_text.encode()).hexdigest():
        raise RuntimeError("AW source digest differs from the executed synthetic tool output")
    if evidence["source_bytes"] != len(expected_text.encode()):
        raise RuntimeError("AW source byte count differs")
    if evidence["adoption"] != "not_observed" or evidence["enforcement"] != "not_attempted":
        raise RuntimeError("Observation-only boundary claimed adoption or enforcement")
    if not evidence["native_payload_preserved"] or len(evidence["calls"]) != 1:
        raise RuntimeError("Expected one real Provider call and preserved payload")
    call = evidence["calls"][0]
    inspection = (call.get("output") or {}).get("inspection", {})
    coverage = inspection.get("coverage", {})
    if case == "provider-failure":
        if (
            call["receipt"]["disposition"] != "failed"
            or call.get("output") is not None
            or evidence["execution"]["decision"] != "preserve"
        ):
            raise RuntimeError(
                "Provider failure did not produce a failed receipt and preserve decision"
            )
        return {
            "status": "passed",
            "case": case,
            "evidence": str(paths[0]),
            "receipt_disposition": "failed",
            "execution_decision": "preserve",
            "provider_fault_injected": True,
            "adoption": "not_observed",
            "enforcement": "not_attempted",
        }
    if evidence["execution"]["decision"] != "proceed":
        raise RuntimeError("Successful inspection did not settle the Core plan")
    if case == "sensitive" and not any(
        finding.get("rule_id") == "api_key" for finding in inspection.get("findings", [])
    ):
        raise RuntimeError("Scanner did not identify the synthetic API-key fixture")
    if call["receipt"]["disposition"] != "produced" or inspection.get("verdict") != case:
        raise RuntimeError("Real Provider did not produce the expected inspection")
    if (
        coverage.get("complete") is not True
        or coverage.get("scanned_bytes") != evidence["source_bytes"]
    ):
        raise RuntimeError("Provider did not report complete coverage of the actual source")
    if (
        coverage.get("input_bytes") != evidence["source_bytes"]
        or coverage.get("input_digest") != evidence["source_digest"]
    ):
        raise RuntimeError("Provider coverage does not bind to the actual source")
    return {
        "status": "passed",
        "case": case,
        "evidence": str(paths[0]),
        "inspection": evidence["calls"][0].get("output"),
        "adoption": "not_observed",
        "enforcement": "not_attempted",
    }


def run(args: argparse.Namespace) -> int:
    root = args.output_dir
    expected_text, tool_command = fixture(args.case)
    root.mkdir(mode=0o700, parents=False, exist_ok=False)
    home, work = root / "isolated-home", root / "work"
    home.mkdir(mode=0o700)
    work.mkdir(mode=0o700)
    (home / "tmp").mkdir()
    codex_home = home / ".codex"
    codex_home.mkdir()
    settings = root / "aw-settings.json"
    hook = shlex.join([str(args.hook_bin), args.host, str(settings)])
    if args.host == "qoder" and args.qoder_existing_login:
        # Only this test's fixed tool event is captured; no historical session is read.
        capture = root / "capture_hook.py"
        capture.write_text(
            "import os,sys\n"
            "raw=sys.stdin.buffer.read(1048577)\n"
            "if len(raw)>1048576: raise SystemExit(1)\n"
            "with open(sys.argv[1],'xb') as f: f.write(raw)\n"
            "with open(sys.argv[1],'rb') as f: os.dup2(f.fileno(),0)\n"
            "os.execv(sys.argv[2],sys.argv[2:])\n"
        )
        hook = shlex.join(
            [
                sys.executable,
                str(capture),
                str(root / "native-event.json"),
                str(args.hook_bin),
                args.host,
                str(settings),
            ]
        )
    hooks = {
        "hooks": {
            "PostToolUse": [
                {
                    "matcher": "Bash" if args.host == "qoder" else "^Bash$",
                    "hooks": [{"type": "command", "command": hook, "timeout": 20}],
                }
            ]
        }
    }
    write_json(
        root / "synthetic-fixture.json",
        {
            "synthetic_input": True,
            "command": tool_command,
            "expected_text": expected_text,
            "case": args.case,
        },
    )
    server = None
    thread = None
    process = None
    lifecycle = {
        "launcher_pid": os.getpid(),
        "authentication": (
            "existing Qoder login used in place; not copied"
            if args.qoder_existing_login
            else "none; isolated empty homes"
        ),
        "expected_lifetime_seconds": 90,
        "output_dir": str(root),
        "working_directory": str(work),
    }
    result = {"status": "failed"}
    lifecycle["case"] = args.case
    lifecycle["sandbox"] = args.sandbox if args.host == "codex" else "not exercised: login probe"
    lifecycle["sandbox_validation"] = (
        "not exercised"
        if args.host == "qoder"
        else (
            "not verified; fixed synthetic command only"
            if args.sandbox == "danger-full-access"
            else "native read-only requested"
        )
    )
    try:
        if args.host == "codex":
            server = FixtureServer(root, tool_command)
            thread = threading.Thread(
                target=server.serve_forever, kwargs={"poll_interval": 0.1}, daemon=True
            )
            thread.start()
            port = server.server_port
            lifecycle.update(
                {
                    "server_pid": os.getpid(),
                    "server_port": port,
                    "server_stop": f"kill -TERM {os.getpid()}",
                }
            )
            (codex_home / "config.toml").write_text(
                'model="gpt-5.5"\nmodel_provider="aw_fixture"\napproval_policy="never"\n'
                f'sandbox_mode="{args.sandbox}"\n'
                '[model_providers.aw_fixture]\nname="AW fixture"\n'
                f'base_url="http://127.0.0.1:{port}/v1"\nwire_api="responses"\n'
                "requires_openai_auth=false\nsupports_websockets=false\nrequest_max_retries=0\n"
                "stream_max_retries=0\nstream_idle_timeout_ms=10000\n"
                "[features]\napps=false\nplugins=false\nhooks=true\n"
            )
            write_json(codex_home / "hooks.json", hooks)
            command = [
                shutil.which("codex") or "codex",
                "exec",
                "--ephemeral",
                "--ignore-rules",
                "--skip-git-repo-check",
                "--dangerously-bypass-hook-trust",
                "--json",
                "-C",
                str(work),
                "Execute the single provided read-only printf command and finish. The API key is an explicitly synthetic scanner fixture.",
            ]
        else:
            (work / ".qoder").mkdir()
            native_settings = work / ".qoder" / "settings.json"
            write_json(native_settings, hooks)
            command = [
                shutil.which("qodercli") or "qodercli",
                "--config-dir",
                str(home / ".qoder"),
                "--setting-sources",
                "project",
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
                f"Run only this read-only synthetic scanner fixture command: {tool_command}. Then finish.",
            ]
            if args.qoder_existing_login:
                # Retain the user's normal authentication root; settings stay per run.
                index = command.index("--config-dir")
                del command[index : index + 2]
        lifecycle["command"] = command
        lifecycle["server_command"] = sys.argv if server else None
        write_json(root / "lifecycle.json", lifecycle)
        print(json.dumps(lifecycle), flush=True)
        with (root / "cli.stdout.log").open("wb") as stdout, (root / "cli.stderr.log").open(
            "wb"
        ) as stderr:
            process = subprocess.Popen(
                command,
                cwd=work,
                env=(
                    qoder_environment(home)
                    if args.qoder_existing_login
                    else isolated_environment(home)
                ),
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                start_new_session=True,
            )
            lifecycle.update(
                {
                    "agent_pid": process.pid,
                    "process_group": process.pid,
                    "agent_stop": f"kill -TERM -- -{process.pid}",
                    "agent_start_ticks": start_ticks(process.pid),
                }
            )
            write_json(settings, make_settings(args, process, root))
            write_json(root / "lifecycle.json", lifecycle)
            print(f"Started {args.host} PID {process.pid}; evidence: {root}", flush=True)
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                observed = os.waitid(os.P_PID, process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
                if observed is not None:
                    returncode = (
                        observed.si_status
                        if observed.si_code == os.CLD_EXITED
                        else -observed.si_status
                    )
                    break
                time.sleep(0.1)
            else:
                raise subprocess.TimeoutExpired(command, 90)
        if args.host == "codex":
            result = validate_codex(root, server, returncode, args.case)
        elif args.qoder_existing_login and returncode == 0:
            native = json.loads((root / "native-event.json").read_text())
            if native.get("tool_name") != "Bash" or native.get("hook_event_name") != "PostToolUse":
                raise RuntimeError("Expected the real Qoder Bash post-tool event")
            response = native["tool_response"]
            if response.get("kind") != "completed" or response.get("exitCode") != 0:
                raise RuntimeError("Qoder tool did not complete successfully")
            result = validate_inspection(root, args.case, response["stdout"])
            result.update(
                real_qoder_hook=True,
                model="Qoder configured model",
                native_turn="launcher-owned single turn",
            )
        else:
            logs = (root / "cli.stdout.log").read_text(errors="replace") + (
                root / "cli.stderr.log"
            ).read_text(errors="replace")
            if any(
                marker in logs.lower()
                for marker in (
                    "not logged",
                    "login",
                    "log in",
                    "sign in",
                    "authentication",
                    "unauthorized",
                )
            ):
                result = {
                    "status": "blocked",
                    "reason": "Isolated Qoder has no authentication; no existing credentials copied",
                    "returncode": returncode,
                    "real_hook_verified": False,
                }
            else:
                raise RuntimeError(
                    "Qoder probe did not establish the expected unauthenticated boundary; inspect logs"
                )
    except (OSError, RuntimeError, subprocess.TimeoutExpired, KeyboardInterrupt) as error:
        result = {"status": "failed", "reason": str(error)}
    finally:
        if process is not None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            # Keep the leader unreaped until the entire owned group is killed.
            time.sleep(0.1)
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
            lifecycle["agent_returncode"] = process.returncode
        if server is not None:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
            lifecycle["server_stopped"] = not thread.is_alive()
        shutil.rmtree(home)
        lifecycle["isolated_agent_home_removed"] = not home.exists()
        lifecycle["finished_at_unix"] = time.time()
        write_json(root / "lifecycle.json", lifecycle)
        write_json(root / "result.json", result)
    print(json.dumps(result, indent=2), flush=True)
    return 0 if result["status"] in ("passed", "blocked") else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, choices=("codex", "qoder"))
    parser.add_argument("--provider-version", required=True)
    parser.add_argument(
        "--qoder-existing-login",
        action="store_true",
        help="Qoder only: use the current login in place; do not copy or delete its configuration",
    )
    parser.add_argument(
        "--case", choices=("sensitive", "clean", "provider-failure"), default="sensitive"
    )
    parser.add_argument(
        "--sandbox",
        choices=("read-only", "danger-full-access"),
        default="read-only",
        help="Danger-full-access is only for the fixed scripted printf fixture; it does not validate sandboxing",
    )
    for name in ("hook-bin", "provider-python", "provider-source", "output-dir"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    args = parser.parse_args()
    if args.qoder_existing_login and args.host != "qoder":
        parser.error("--qoder-existing-login requires --host qoder")
    for name in ("hook_bin", "provider_python", "provider_source", "output_dir"):
        if not getattr(args, name).is_absolute():
            parser.error(f"--{name.replace('_', '-')} must be absolute")
    for name in ("hook_bin", "provider_python", "provider_source"):
        if not getattr(args, name).exists():
            parser.error(f"--{name.replace('_', '-')} does not exist")

    def interrupted(_signal: int, _frame: Any) -> None:
        raise KeyboardInterrupt("Smoke interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    os.umask(0o077)
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
