#!/usr/bin/env python3
"""Validate the official QwenPaw app entrypoint with isolated native state.

Run separately for allow and block with a new --output directory each time.
The default model is deterministic and local. --key-file uses real Token Plan
through a test-only relay that keeps credentials outside the Agent profile.
External middleware is loaded by the app lifespan; ACP/TUI omit that loader.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import sys
import time
import urllib.error
import urllib.request

SPEC = importlib.util.spec_from_file_location(
    "qwenpaw_acp_fixture", Path(__file__).with_name("run-acp.py")
)
ACP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ACP)
REPO = ACP.REPO

EXISTING_PLUGIN = '''"""Independent observer installed before AW takes ownership of its plugin."""
import json
import os
from pathlib import Path
from agentscope.middleware import MiddlewareBase
from agentscope.tool import ToolResponse

def record(value):
    with Path(os.environ["AW_COEXIST_TRACE"]).open("a") as output:
        output.write(json.dumps(value) + "\\n")

class Observer(MiddlewareBase):
    async def on_acting(self, agent, input_kwargs, next_handler):
        call = input_kwargs["tool_call"]
        record({"stage":"before","tool":call.name,"call_id":call.id})
        async for response in next_handler():
            if isinstance(response, ToolResponse):
                record({"stage":"after","tool":call.name,"call_id":call.id,"response":response.model_dump(mode="json")})
            yield response

class Plugin:
    def register(self, api):
        api.register_middleware(lambda ctx, cfg: Observer(), priority=25)
        record({"stage":"registered"})

plugin = Plugin()
'''


def run(args: argparse.Namespace) -> None:
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ("home", "home/secret", "work", "state"):
        (output / name).mkdir()
    real_model = args.key_file is not None
    model_name = "qwen3.7-plus" if real_model else "aw-fixture"
    preserved: dict[str, str] = {}
    if real_model:
        plugin = output / "home/plugins/existing-observer"
        plugin.mkdir(parents=True)
        ACP.write(
            plugin / "plugin.json",
            {
                "id": "existing-observer",
                "name": "Existing observer fixture",
                "version": "0.1.0",
                "type": "general",
                "entry": {"backend": "plugin.py"},
                "dependencies": [],
            },
        )
        (plugin / "plugin.py").write_text(EXISTING_PLUGIN)
        (output / "work/preserved-state.txt").write_text("State existed before AW launch.\n")
        for path in [
            plugin / "plugin.json",
            plugin / "plugin.py",
            output / "work/preserved-state.txt",
        ]:
            preserved[str(path.relative_to(output))] = hashlib.sha256(path.read_bytes()).hexdigest()
    ACP.write(
        output / "home/config.json",
        {
            "agents": {
                "active_agent": "default",
                "profiles": {"default": {"id": "default", "workspace_dir": str(output / "work")}},
            },
            "plugins": {
                "aw-native": {"enabled": True},
                **({"existing-observer": {"enabled": True}} if real_model else {}),
            },
        },
    )
    ACP.write(
        output / "work/agent.json",
        {
            "id": "default",
            "name": "AW fixture",
            "workspace_dir": str(output / "work"),
            "active_model": {"provider_id": "aw-fixture", "model": model_name},
            "running": {
                "max_iters": 3,
                "auto_title_config": {"enabled": False},
                "llm_retry_enabled": False,
            },
            "channels": {"console": {"enabled": True}},
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
        QWENPAW_DISABLE_KEYRING="true",
        QWENPAW_AUTH_ENABLED="false",
        PYTHONDONTWRITEBYTECODE="1",
        AW_COEXIST_TRACE=str(output / "coexist.jsonl"),
    )
    processes = ACP.Processes(output, environment)
    report = {
        "entrypoint": "official qwenpaw app",
        "model": (model_name if real_model else "local deterministic OpenAI-compatible fixture"),
        "real_model": real_model,
        "mode": args.mode,
        "passed": False,
    }
    socket_path = output / "aw.sock"
    ports = []
    try:
        proxy_command = (
            [
                sys.executable,
                str(Path(__file__).with_name("token-plan-proxy.py").resolve()),
                "--output",
                str(output),
                "--key-file",
                str(args.key_file),
            ]
            if real_model
            else [
                sys.executable,
                str(Path(__file__).with_name("run-acp.py").resolve()),
                "--model-server",
                "--output",
                str(output),
            ]
        )
        processes.start(
            proxy_command,
            "model",
        )
        deadline = time.monotonic() + 5
        while not (output / "model-server.json").exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        model = json.loads((output / "model-server.json").read_text())
        processes.records[0]["ports"] = [model["port"]]
        processes.save()
        ports.append(model["port"])
        # This documented legacy profile is migrated by the stock provider
        # manager. The placeholder key is solely for our loopback model.
        ACP.write(
            output / "home/secret/providers.json",
            {
                "custom_providers": {
                    "aw-fixture": {
                        "name": "AW local fixture",
                        "base_url": f"http://127.0.0.1:{model['port']}/v1",
                        "api_key": "aw-local-fixture",
                        "chat_model": "OpenAIChatModel",
                        "models": [
                            {
                                "id": model_name,
                                "name": "AW local fixture",
                                "supports_tool_calling": True,
                                "generate_kwargs": {"extra_body": {"enable_thinking": False}},
                            }
                        ],
                    }
                },
                "active_llm": {"provider_id": "aw-fixture", "model": model_name},
            },
        )
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        ports.append(port)
        config = {
            "apiVersion": "aw/v1alpha1",
            "kind": "AWConfiguration",
            "metadata": {"name": "qwenpaw-app-fixture"},
            "spec": {
                "daemon": {
                    "startup": "external",
                    "endpoint": "auto",
                    "state_dir": "auto",
                },
                "execution": {
                    "guarantee": "native_hook",
                    "default_event_budget_ms": 5000,
                },
                "audit": {"enabled": True, "payload": "metadata_only"},
                "agents": {
                    "qwenpaw": {
                        "adapter": "qwenpaw",
                        "argv": [
                            str(args.runtime.resolve() / "venv/bin/qwenpaw"),
                            "app",
                            "--host",
                            "127.0.0.1",
                            "--port",
                            str(port),
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
                        "config": (
                            {"blocked_substrings": ["AW_DENY_FIXTURE"]}
                            if real_model
                            else {
                                "blocked_tools": (
                                    ["execute_shell_command"] if args.mode == "block" else []
                                )
                            }
                        ),
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
        ACP.write(output / "aw.json", config)
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
        processes.start(
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
            ports=[port],
        )
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{port}/api/plugins", timeout=2
                ) as response:
                    plugins = json.load(response)
                entries = plugins if isinstance(plugins, list) else plugins.get("plugins", [])
                registered = next(
                    (item for item in entries if item["id"] == "aw-native" and item["loaded"]),
                    None,
                )
                if registered:
                    report["plugin"] = registered
                    if real_model:
                        report["existing_plugin"] = next(
                            (
                                item
                                for item in entries
                                if item["id"] == "existing-observer"
                                and item["loaded"]
                                and item["enabled"]
                            ),
                            None,
                        )
                        if report["existing_plugin"] is None:
                            time.sleep(0.2)
                            continue
                    break
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.2)
        else:
            raise TimeoutError("Native AW plugin did not become loaded")
        marker = "AW_DENY_FIXTURE.txt" if real_model and args.mode == "block" else "answer.txt"
        instruction = (
            "This is an isolated tool-hook acceptance test. Call "
            "execute_shell_command exactly once with this exact command:\n"
            f"```sh\nprintf '42\\n' > {marker}; cat {marker}\n```\n"
            "Do not use other tools, read workspace instructions, retry, delegate, "
            "or change this command. If denied, report the denial and stop. "
            "Otherwise report the exact tool stdout and stop."
        )
        payload = {
            "session_id": "aw-entrypoint-fixture",
            "user_id": "aw-fixture",
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": (
                                instruction
                                if real_model
                                else "Use the native shell once for this isolated AW Hook fixture, then stop."
                            ),
                        }
                    ],
                }
            ],
        }
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/console/chat",
            data=json.dumps(payload).encode(),
            headers={"Content-Type": "application/json", "X-Agent-Id": "default"},
        )
        with urllib.request.urlopen(request, timeout=120 if real_model else 60) as response:
            stream = response.read(1024 * 1024 + 1)
        if len(stream) > 1024 * 1024:
            raise RuntimeError("Console response exceeds fixture bound")
        (output / "console-events.log").write_bytes(stream)
        audit = [json.loads(line) for line in (output / "audit.jsonl").read_text().splitlines()]
        report["audit"] = audit
        report["marker_exists"] = (output / "work" / marker).exists()
        report["marker_value"] = (
            (output / "work" / marker).read_text() if report["marker_exists"] else None
        )
        report["passed"] = report["marker_exists"] == (args.mode == "allow") and any(
            item.get("event") == "tool.before"
            and item.get("disposition") == ("block" if args.mode == "block" else "observe")
            for item in audit
        )
        if args.mode == "allow":
            report["passed"] = (
                report["passed"]
                and report["marker_value"] == "42\n"
                and any(item.get("event") == "tool.after" for item in audit)
            )
        if real_model:
            report["coexist"] = [
                json.loads(line) for line in (output / "coexist.jsonl").read_text().splitlines()
            ]
            report["model_requests"] = [
                json.loads(line)
                for line in (output / "model-requests.jsonl").read_text().splitlines()
            ]
            report["passed"] = report["passed"] and all(
                any(item["stage"] == stage for item in report["coexist"])
                for stage in ["registered", "before", "after"]
            )
            if args.mode == "allow":
                report["passed"] = report["passed"] and any(
                    item["tool_result_contains_42"] for item in report["model_requests"]
                )
            else:
                report["passed"] = (
                    report["passed"]
                    and any(
                        item["stage"] == "after" and item["response"]["state"] == "denied"
                        for item in report["coexist"]
                    )
                    and any(item["tool_result_count"] for item in report["model_requests"])
                )
            report["registration_receipts"] = []
            for path in (output / "state").rglob("*.json"):
                value = json.loads(path.read_text())
                if (
                    value.get("adapter") == "qwenpaw"
                    and value.get("version") == 1
                    and "token" in value
                ):
                    report["registration_receipts"].append(
                        {key: value[key] for key in ["version", "adapter", "pid", "hooks"]}
                    )
            report["passed"] = report["passed"] and any(
                item["hooks"] == 2 for item in report["registration_receipts"]
            )
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        processes.close()
        report["socket_removed"] = not socket_path.exists()
        report["plugin_removed"] = not (output / "home/plugins/aw-native").exists()
        if real_model:
            report["preserved_files"] = {
                name: (output / name).is_file()
                and hashlib.sha256((output / name).read_bytes()).hexdigest() == digest
                for name, digest in preserved.items()
            }
            report["passed"] = report["passed"] and all(report["preserved_files"].values())
            key = args.key_file.read_bytes().strip()
            matches = []
            for path in output.rglob("*"):
                if path.is_file() and key and key in path.read_bytes():
                    matches.append(str(path.relative_to(output)))
                    path.write_bytes(path.read_bytes().replace(key, b"[REDACTED]"))
            report["credential_scan"] = {
                "matches_redacted": matches,
                "clean": not matches,
            }
        report["ports_closed"] = []
        for port in ports:
            with socket.socket() as connection:
                connection.settimeout(0.2)
                report["ports_closed"].append(
                    {
                        "port": port,
                        "closed": connection.connect_ex(("127.0.0.1", port)) != 0,
                    }
                )
        ACP.write(output / "result.json", report)
    if not report["passed"]:
        raise RuntimeError("App entrypoint did not meet tool/adoption assertions")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=["allow", "block"], required=True)
    parser.add_argument("--runtime", type=Path, default=REPO / "target/native-lab/qwenpaw")
    parser.add_argument("--aw", type=Path, default=REPO / "src/aw/target/debug/aw")
    parser.add_argument(
        "--key-file",
        type=Path,
        help="Use real Token Plan through an in-memory credential relay",
    )
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(parser.parse_args())
