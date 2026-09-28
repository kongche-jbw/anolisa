#!/usr/bin/env python3
"""Validate the official QwenPaw app entrypoint using a local model fixture.

Run separately for allow and block with a new --output directory each time.
No model credentials or external model calls are used. External middleware is
loaded by the app lifespan; ACP/TUI currently omit that external plugin loader.
"""

from __future__ import annotations

import argparse
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


def run(args: argparse.Namespace) -> None:
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    for name in ("home", "home/secret", "work", "state"):
        (output / name).mkdir()
    ACP.write(
        output / "home/config.json",
        {
            "agents": {
                "active_agent": "default",
                "profiles": {
                    "default": {"id": "default", "workspace_dir": str(output / "work")}
                },
            },
            "plugins": {"aw-native": {"enabled": True}},
        },
    )
    ACP.write(
        output / "work/agent.json",
        {
            "id": "default",
            "name": "AW fixture",
            "workspace_dir": str(output / "work"),
            "active_model": {"provider_id": "aw-fixture", "model": "aw-fixture"},
            "running": {"max_iters": 3},
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
    )
    processes = ACP.Processes(output, environment)
    report = {
        "entrypoint": "official qwenpaw app",
        "model": "local deterministic OpenAI-compatible fixture",
        "mode": args.mode,
        "passed": False,
    }
    socket_path = output / "aw.sock"
    ports = []
    try:
        processes.start(
            [
                sys.executable,
                str(Path(__file__).with_name("run-acp.py").resolve()),
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
                                "id": "aw-fixture",
                                "name": "AW local fixture",
                                "supports_tool_calling": True,
                            }
                        ],
                    }
                },
                "active_llm": {"provider_id": "aw-fixture", "model": "aw-fixture"},
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
                        "config": {
                            "blocked_tools": (
                                ["execute_shell_command"]
                                if args.mode == "block"
                                else []
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
                entries = (
                    plugins if isinstance(plugins, list) else plugins.get("plugins", [])
                )
                registered = next(
                    (
                        item
                        for item in entries
                        if item["id"] == "aw-native" and item["loaded"]
                    ),
                    None,
                )
                if registered:
                    report["plugin"] = registered
                    break
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(0.2)
        else:
            raise TimeoutError("Native AW plugin did not become loaded")
        payload = {
            "session_id": "aw-entrypoint-fixture",
            "user_id": "aw-fixture",
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Use the native shell once for this isolated AW Hook fixture, then stop.",
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
        with urllib.request.urlopen(request, timeout=60) as response:
            stream = response.read(1024 * 1024 + 1)
        if len(stream) > 1024 * 1024:
            raise RuntimeError("Console response exceeds fixture bound")
        (output / "console-events.log").write_bytes(stream)
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
            item.get("event") == "tool.before"
            and item.get("disposition")
            == ("block" if args.mode == "block" else "observe")
            for item in audit
        )
        if args.mode == "allow":
            report["passed"] = (
                report["passed"]
                and report["marker_value"] == "42\n"
                and any(item.get("event") == "tool.after" for item in audit)
            )
    except BaseException as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        processes.close()
        report["socket_removed"] = not socket_path.exists()
        report["plugin_removed"] = not (output / "home/plugins/aw-native").exists()
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
    parser.add_argument(
        "--runtime", type=Path, default=REPO / "target/native-lab/qwenpaw"
    )
    parser.add_argument("--aw", type=Path, default=REPO / "src/aw/target/debug/aw")
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(parser.parse_args())
