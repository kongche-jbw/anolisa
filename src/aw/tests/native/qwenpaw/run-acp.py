#!/usr/bin/env python3
"""Exercise the installed QwenPaw ACP CLI through AW with a local model fixture.

This diagnostic exposed missing external-plugin loading in the official ACP
entrypoint. Current AW rejects that route; it is not a passing acceptance test.
The local model fixture is also reused by the supported App-entrypoint test.
All configuration, ports, processes, and evidence belong to --output.
"""

from __future__ import annotations

import argparse
import http.server
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[5]


def write(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2) + "\n")


def model_server(output: Path) -> None:
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def do_POST(self) -> None:
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            tools = [item["function"]["name"] for item in body.get("tools", [])]
            tool_seen = any(message["role"] == "tool" for message in body["messages"])
            with (output / "model-requests.jsonl").open("a") as log:
                log.write(
                    json.dumps(
                        {
                            "path": self.path,
                            "tools": tools,
                            "tool_result_seen": tool_seen,
                            "stream": body.get("stream"),
                        }
                    )
                    + "\n"
                )
            if not tool_seen:
                if "execute_shell_command" not in tools:
                    self.send_error(400, "Native shell tool missing from request")
                    return
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_aw_fixture",
                            "type": "function",
                            "function": {
                                "name": "execute_shell_command",
                                "arguments": json.dumps(
                                    {
                                        "command": "printf '42\\n' > answer.txt",
                                        "timeout": 5,
                                    }
                                ),
                            },
                        }
                    ],
                }
                finish = "tool_calls"
            else:
                delta, finish = {
                    "role": "assistant",
                    "content": "AW fixture completed.",
                }, "stop"
            base = {"id": "chatcmpl-aw-fixture", "created": 1, "model": "aw-fixture"}
            if body.get("stream"):
                frames = [
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [
                            {"index": 0, "delta": delta, "finish_reason": None}
                        ],
                    },
                    {
                        **base,
                        "object": "chat.completion.chunk",
                        "choices": [{"index": 0, "delta": {}, "finish_reason": finish}],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2,
                        },
                    },
                ]
                payload = (
                    "".join("data: " + json.dumps(frame) + "\n\n" for frame in frames)
                    + "data: [DONE]\n\n"
                ).encode()
                content_type = "text/event-stream"
            else:
                for call in delta.get("tool_calls", []):
                    call.pop("index", None)
                payload = json.dumps(
                    {
                        **base,
                        "object": "chat.completion",
                        "choices": [
                            {"index": 0, "message": delta, "finish_reason": finish}
                        ],
                        "usage": {
                            "prompt_tokens": 1,
                            "completion_tokens": 1,
                            "total_tokens": 2,
                        },
                    }
                ).encode()
                content_type = "application/json"
            self.send_response(200)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    with http.server.HTTPServer(("127.0.0.1", 0), Handler) as server:
        server.timeout = 1
        write(
            output / "model-server.json",
            {"pid": os.getpid(), "port": server.server_port},
        )
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            server.handle_request()


class Processes:
    def __init__(self, output: Path, environment: dict[str, str]) -> None:
        self.output, self.environment = output, environment
        self.records: list[dict] = []
        self.children: list[tuple[subprocess.Popen, object, dict]] = []

    def save(self) -> None:
        write(self.output / "processes.json", self.records)

    def start(
        self,
        command: list[str],
        name: str,
        *,
        protocol: bool = False,
        ports: list[int] | None = None,
    ) -> subprocess.Popen:
        log = (self.output / f"{name}.log").open("wb")
        record = {
            "name": name,
            "command": command,
            "cwd": str(self.output / "work"),
            "pid": None,
            "pgid": None,
            "ports": ports or [],
            "log": str(self.output / f"{name}.log"),
            "deadline_seconds": 240,
            "stop": None,
        }
        self.records.append(record)
        self.save()
        child = subprocess.Popen(
            command,
            cwd=self.output / "work",
            env=self.environment,
            stdin=subprocess.PIPE if protocol else subprocess.DEVNULL,
            stdout=subprocess.PIPE if protocol else log,
            stderr=log,
            start_new_session=True,
        )
        record.update(pid=child.pid, pgid=child.pid, stop=f"kill -TERM -- -{child.pid}")
        self.children.append((child, log, record))
        self.save()
        return child

    def close(self) -> None:
        for child, log, record in reversed(self.children):
            if child.stdin:
                child.stdin.close()
            # Keep leader unreaped until group signalling is complete so the
            # numeric group remains owned even after a normal CLI exit.
            try:
                os.killpg(child.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                exited = os.waitid(
                    os.P_PID, child.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT
                )
                if exited is not None:
                    break
                time.sleep(0.05)
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=5)
            if child.stdout:
                child.stdout.close()
            log.close()
            record.update(
                exit_code=child.returncode,
                pid_absent=not Path(f"/proc/{child.pid}").exists(),
            )
            self.save()


class Acp:
    def __init__(self, child: subprocess.Popen, output: Path) -> None:
        self.child, self.output = child, output
        self.buffer = b""
        self.serial = 0

    def send(self, value: dict) -> None:
        self.child.stdin.write(json.dumps(value).encode() + b"\n")
        self.child.stdin.flush()

    def request(self, method: str, params: dict, timeout: int = 90) -> dict:
        self.serial += 1
        identifier = self.serial
        self.send(
            {"jsonrpc": "2.0", "id": identifier, "method": method, "params": params}
        )
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if b"\n" not in self.buffer:
                ready, _, _ = select.select(
                    [self.child.stdout], [], [], min(1, deadline - time.monotonic())
                )
                if not ready:
                    continue
                data = os.read(self.child.stdout.fileno(), 65536)
                if not data:
                    raise RuntimeError("QwenPaw ACP exited before replying")
                self.buffer += data
                if len(self.buffer) > 4 * 1024 * 1024:
                    raise RuntimeError("ACP frame exceeds fixture bound")
                continue
            line, self.buffer = self.buffer.split(b"\n", 1)
            message = json.loads(line)
            with (self.output / "acp-events.jsonl").open("a") as log:
                log.write(json.dumps(message) + "\n")
            if message.get("method") == "session/request_permission":
                options = message["params"]["options"]
                option = next(
                    option for option in options if option["kind"] == "allow_once"
                )
                self.send(
                    {
                        "jsonrpc": "2.0",
                        "id": message["id"],
                        "result": {
                            "outcome": {
                                "outcome": "selected",
                                "optionId": option["optionId"],
                            }
                        },
                    }
                )
            elif message.get("id") == identifier and "method" not in message:
                if "error" in message:
                    raise RuntimeError(f"ACP request failed: {message['error']}")
                return message["result"]
        raise TimeoutError(f"ACP {method} exceeded {timeout} seconds")


def run(args: argparse.Namespace) -> None:
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ("home", "work", "state"):
        (output / name).mkdir()
    write(
        output / "home/config.json",
        {
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": str(output / "work")}
                },
            }
        },
    )
    write(
        output / "work/agent.json",
        {
            "id": "default",
            "name": "AW fixture",
            "workspace_dir": str(output / "work"),
            "running": {"max_iters": 3},
        },
    )
    environment = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith(("AW_", "QWENPAW_", "OPENAI_"))
    }
    environment.update(
        QWENPAW_WORKING_DIR=str(output / "home"),
        QWENPAW_SECRET_DIR=str(output / "home/secret"),
        PYTHONDONTWRITEBYTECODE="1",
        OPENAI_API_KEY="aw-local-fixture",
        OPENAI_MODEL="aw-fixture",
        PAW_DISABLE_BACKEND_WARMUP="1",
    )
    socket_path = output / "aw.sock"
    config = {
        "apiVersion": "aw/v1alpha1",
        "kind": "AWConfiguration",
        "metadata": {"name": "qwenpaw-acp-fixture"},
        "spec": {
            "daemon": {"startup": "external", "endpoint": "auto", "state_dir": "auto"},
            "execution": {"guarantee": "native_hook", "default_event_budget_ms": 5000},
            "audit": {"enabled": True, "payload": "metadata_only"},
            "agents": {
                "qwenpaw": {
                    "adapter": "qwenpaw",
                    "argv": [
                        str(args.runtime.resolve() / "venv/bin/qwenpaw"),
                        "acp",
                        "--workspace",
                        str(output / "work"),
                        "--runtime-provider",
                        "openai-env",
                        "--local-diagnostics",
                    ],
                }
            },
            "providers": {
                "policy": {
                    "protocol": "aw-provider/v1alpha1",
                    "transport": {
                        "type": "stdio",
                        "location": "agent",
                        "argv": [
                            sys.executable,
                            str(REPO / "src/aw/examples/providers/policy.py"),
                        ],
                    },
                    "timeout_ms": 2000,
                    "max_output_bytes": 1048576,
                    "config": {
                        "blocked_tools": (
                            ["execute_shell_command"] if args.mode == "block" else []
                        )
                    },
                }
            },
            "events": {
                "tool.before": {
                    "enabled": True,
                    "required": True,
                    "steps": [
                        {
                            "id": "before",
                            "provider": "policy",
                            "operation": "check",
                            "effects": ["observe", "block"],
                            "on_error": "block",
                        }
                    ],
                },
                "tool.after": {
                    "enabled": True,
                    "required": True,
                    "steps": [
                        {
                            "id": "after",
                            "provider": "policy",
                            "operation": "record",
                            "effects": ["observe"],
                            "on_error": "report",
                        }
                    ],
                },
            },
        },
    }
    write(output / "aw.json", config)
    processes = Processes(output, environment)
    report = {
        "model": "local deterministic OpenAI-compatible fixture",
        "entrypoint": "official qwenpaw acp",
        "mode": args.mode,
        "passed": False,
    }
    try:
        processes.start(
            [
                sys.executable,
                str(Path(__file__).resolve()),
                "--model-server",
                "--output",
                str(output),
            ],
            "model",
        )
        deadline = time.monotonic() + 5
        while (
            not (output / "model-server.json").exists() and time.monotonic() < deadline
        ):
            time.sleep(0.05)
        model = json.loads((output / "model-server.json").read_text())
        processes.records[0]["ports"] = [model["port"]]
        processes.save()
        environment["OPENAI_BASE_URL"] = f"http://127.0.0.1:{model['port']}/v1"
        processes.start(
            [
                str(args.aw.resolve()),
                "serve",
                "--config",
                str(output / "aw.json"),
                "--socket",
                str(socket_path),
            ],
            "daemon",
        )
        deadline = time.monotonic() + 5
        while not socket_path.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        if not socket_path.exists():
            raise TimeoutError("AW daemon did not become ready")
        child = processes.start(
            [
                str(args.aw.resolve()),
                "run",
                "qwenpaw",
                "--config",
                str(output / "aw.json"),
                "--state-dir",
                str(output / "state"),
                "--socket",
                str(socket_path),
            ],
            "agent",
            protocol=True,
        )
        acp = Acp(child, output)
        report["initialize"] = acp.request(
            "initialize",
            {
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "aw-entrypoint-fixture", "version": "1"},
            },
        )
        session = acp.request(
            "session/new", {"cwd": str(output / "work"), "mcpServers": []}
        )
        report["session_id"] = session["sessionId"]
        report["prompt"] = acp.request(
            "session/prompt",
            {
                "sessionId": session["sessionId"],
                "prompt": [
                    {
                        "type": "text",
                        "text": "Run the single requested shell tool once, then stop. This is an isolated AW Hook fixture.",
                    }
                ],
            },
            timeout=120,
        )
        audit = [
            json.loads(line)
            for line in (output / "audit.jsonl").read_text().splitlines()
        ]
        report["audit"] = audit
        report["marker_exists"] = (output / "work/answer.txt").exists()
        report["marker_value"] = (
            (output / "work/answer.txt").read_text()
            if report["marker_exists"]
            else None
        )
        report["passed"] = report["marker_exists"] == (args.mode == "allow") and any(
            item.get("event") == "tool.before" for item in audit
        )
        if args.mode == "allow":
            report["passed"] = report["passed"] and any(
                item.get("event") == "tool.after" for item in audit
            )
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        processes.close()
        report["socket_removed"] = not socket_path.exists()
        report["plugin_removed"] = not (output / "home/plugins/aw-native").exists()
        write(output / "result.json", report)
    if not report["passed"]:
        raise RuntimeError("Entrypoint fixture did not meet tool/adoption assertions")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-server", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=["allow", "block"], default="allow")
    parser.add_argument(
        "--runtime", type=Path, default=REPO / "target/native-lab/qwenpaw"
    )
    parser.add_argument("--aw", type=Path, default=REPO / "src/aw/target/debug/aw")
    arguments = parser.parse_args()
    if arguments.model_server:
        model_server(arguments.output)
    else:
        signal.signal(
            signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt())
        )
        run(arguments)
