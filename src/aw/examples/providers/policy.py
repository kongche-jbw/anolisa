#!/usr/bin/env python3
"""Example structured Provider with optional daemon-independent sec-core CLI use.

AW supplies one JSON request on stdin and consumes one JSON response. This sample
keeps policy choices separate from host Hook response formats. Its substring rule
is deliberately a text-matching example, not a shell parser or a security sandbox.
Set sec_core.argv to an explicit CLI prefix; V2 requires a separately managed
sec-core daemon, while the installed V1 CLI performs regex scans locally.
"""

from __future__ import annotations

import json
import os
import selectors
import subprocess
import sys
import time
from typing import Any

API_VERSION = "aw-provider/v1alpha1"
MAX_BYTES = 1024 * 1024
SHELL_TOOLS = {
    "qoder": "Bash",
    "openclaw": "exec",
    "hermes": "terminal",
    "qwenpaw": "execute_shell_command",
}
OPERATIONS = [
    {"name": "check", "events": ["tool.before"], "effects": ["observe", "block"]},
    {"name": "record", "events": ["tool.after"], "effects": ["observe"]},
]


class PolicyError(Exception):
    """A fixed diagnostic code that never includes event or command content."""


def _object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON field")
        value[key] = item
    return value


def _invalid_number(_value: str) -> None:
    raise ValueError("non-finite JSON number")


def _json(raw: bytes | bytearray) -> Any:
    return json.loads(raw, object_pairs_hook=_object, parse_constant=_invalid_number)


def _require(condition: bool, code: str) -> None:
    if not condition:
        raise PolicyError(code)


def _strings(value: Any, *, nonempty: bool = False) -> list[str]:
    _require(isinstance(value, list) and len(value) <= 128, "invalid_config")
    _require(not nonempty or bool(value), "invalid_config")
    _require(
        all(
            isinstance(item, str) and 0 < len(item) <= 4096 and "\0" not in item
            for item in value
        ),
        "invalid_config",
    )
    return value


def validate_config(config: Any) -> dict[str, Any]:
    """Validate this Provider's private fields without starting a subprocess."""
    _require(isinstance(config, dict), "invalid_config")
    _require(
        set(config) <= {"blocked_tools", "blocked_substrings", "sec_core"},
        "invalid_config",
    )
    _strings(config.get("blocked_tools", []))
    _strings(config.get("blocked_substrings", []))
    if "sec_core" in config:
        scanner = config["sec_core"]
        _require(isinstance(scanner, dict), "invalid_config")
        _require(
            set(scanner) <= {"argv", "timeout_ms", "block_verdicts"}, "invalid_config"
        )
        argv = _strings(scanner.get("argv"), nonempty=True)
        _require(
            os.path.isabs(argv[0])
            and os.path.isfile(argv[0])
            and os.access(argv[0], os.X_OK),
            "invalid_config",
        )
        timeout = scanner.get("timeout_ms", 2000)
        _require(type(timeout) is int and 0 < timeout <= 300000, "invalid_config")
        verdicts = _strings(
            scanner.get("block_verdicts", ["warn", "deny"]), nonempty=True
        )
        _require(
            set(verdicts) <= {"warn", "deny"} and len(set(verdicts)) == len(verdicts),
            "invalid_config",
        )
    return config


def _scan(command: str, config: dict[str, Any], budget_ms: int, started: float) -> bool:
    # Keep the caller's deadline authoritative; reserve time to form a response.
    remaining_ms = budget_ms - (time.monotonic() - started) * 1000 - 50
    _require(remaining_ms > 0, "budget_exhausted")
    timeout = min(config.get("timeout_ms", 2000), remaining_ms) / 1000
    argv = [
        *config["argv"],
        "scan-code",
        "--code",
        command,
        "--language",
        "bash",
        "--mode",
        "regex",
    ]
    _require("\0" not in command, "invalid_tool_input")
    try:
        child = subprocess.Popen(
            argv,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
    except OSError as error:
        raise PolicyError("scanner_unavailable") from error
    # Stay in AW's owned process group so cancellation also kills the scanner.
    # Drain both streams under a deadline; neither memory nor disk can grow
    # beyond the explicit per-stream cap, even when a scanner floods stderr.
    stdout, stderr = bytearray(), bytearray()
    deadline = time.monotonic() + timeout
    try:
        with selectors.DefaultSelector() as selector:
            for stream, buffer in ((child.stdout, stdout), (child.stderr, stderr)):
                os.set_blocking(stream.fileno(), False)
                selector.register(stream, selectors.EVENT_READ, buffer)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                _require(remaining > 0, "scanner_timeout")
                for key, _ in selector.select(timeout=remaining):
                    chunk = os.read(key.fd, 8192)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    _require(
                        len(key.data) + len(chunk) <= MAX_BYTES, "scanner_output_limit"
                    )
                    key.data.extend(chunk)
            try:
                code = child.wait(timeout=max(0, deadline - time.monotonic()))
            except subprocess.TimeoutExpired as error:
                raise PolicyError("scanner_timeout") from error
        _require(code == 0, "scanner_failed")
        try:
            result = _json(stdout)
        except (ValueError, UnicodeError) as error:
            raise PolicyError("invalid_scanner_output") from error
    finally:
        if child.poll() is None:
            child.kill()
        child.wait(timeout=5)
        child.stdout.close()
        child.stderr.close()
    _require(
        isinstance(result, dict) and result.get("ok") is True, "invalid_scanner_output"
    )
    _require(
        isinstance(result.get("verdict"), str)
        and result["verdict"] in {"pass", "warn", "deny"},
        "invalid_scanner_output",
    )
    _require(isinstance(result.get("findings"), list), "invalid_scanner_output")
    return result["verdict"] in config.get("block_verdicts", ["warn", "deny"])


def invoke(
    request: dict[str, Any], config: dict[str, Any], started: float
) -> list[dict[str, str]]:
    """Evaluate a normalized event; control effects require caller permission."""
    for field in ("config_revision", "input_digest"):
        _require(
            isinstance(request.get(field), str) and 0 < len(request[field]) <= 4096,
            "invalid_request",
        )
    budget = request.get("budget_ms")
    _require(type(budget) is int and 0 < budget <= 300000, "invalid_request")
    allowed = request.get("allowed_effects")
    _require(
        isinstance(allowed, list) and all(isinstance(item, str) for item in allowed),
        "invalid_request",
    )
    _require(set(allowed) <= {"observe", "block"}, "unsupported_effect")
    event = request.get("event")
    _require(isinstance(event, dict), "invalid_event")
    operation = request.get("operation")
    _require(
        isinstance(operation, str) and operation in {"check", "record"},
        "unsupported_operation",
    )
    expected = "tool.before" if operation == "check" else "tool.after"
    _require(event.get("name") == expected, "unsupported_event")
    tool = event.get("tool")
    _require(
        isinstance(tool, dict) and isinstance(tool.get("input"), dict), "invalid_event"
    )
    _require(isinstance(tool.get("name"), str) and bool(tool["name"]), "invalid_event")
    native_name = tool.get("native_name", tool["name"])
    _require(isinstance(native_name, str) and bool(native_name), "invalid_event")
    agent = event.get("agent")
    _require(
        isinstance(agent, dict) and isinstance(agent.get("adapter"), str),
        "invalid_event",
    )
    if operation == "record":
        _require("observe" in allowed, "effect_not_allowed")
        return [{"type": "observe"}]
    blocked = bool({tool["name"], native_name} & set(config.get("blocked_tools", [])))
    serialized = json.dumps(tool["input"], ensure_ascii=False, sort_keys=True)
    blocked = blocked or any(
        text in serialized for text in config.get("blocked_substrings", [])
    )
    if (
        not blocked
        and "sec_core" in config
        and native_name == SHELL_TOOLS.get(agent["adapter"])
    ):
        command = tool["input"].get("command")
        _require(
            isinstance(command, str) and bool(command.strip()), "invalid_tool_input"
        )
        blocked = _scan(command, config["sec_core"], budget, started)
    if blocked:
        _require("block" in allowed, "effect_not_allowed")
        return [{"type": "block", "reason_code": "policy_match"}]
    return []


def respond(request: Any, started: float) -> dict[str, Any]:
    """Bind every valid response to the opaque request identifier and digest."""
    response: dict[str, Any] = {
        "api_version": API_VERSION,
        "request_id": "",
        "status": "error",
    }
    try:
        _require(isinstance(request, dict), "invalid_request")
        identifier = request.get("request_id")
        _require(
            isinstance(identifier, str) and 0 < len(identifier) <= 256,
            "invalid_request",
        )
        response["request_id"] = identifier
        _require(request.get("api_version") == API_VERSION, "unsupported_api_version")
        method = request.get("method")
        common = {"api_version", "method", "request_id"}
        fields = {
            "describe": common,
            "validate_config": common | {"config"},
            "invoke": common
            | {
                "operation",
                "config",
                "config_revision",
                "budget_ms",
                "allowed_effects",
                "input_digest",
                "event",
            },
        }
        _require(isinstance(method, str) and method in fields, "unsupported_method")
        _require(set(request) <= fields[method], "invalid_request")
        if method == "describe":
            response["operations"] = OPERATIONS
        else:
            config = validate_config(request.get("config"))
            if method == "invoke":
                response["effects"] = invoke(request, config, started)
                response["input_digest"] = request["input_digest"]
        response["status"] = "ok"
    except PolicyError as error:
        response["error_code"] = str(error)
    return response


def main() -> None:
    """Read exactly one bounded JSON value, never log its contents."""
    started = time.monotonic()
    try:
        raw = sys.stdin.buffer.read(MAX_BYTES + 1)
        _require(len(raw) <= MAX_BYTES, "request_too_large")
        request = _json(raw)
        response = respond(request, started)
    except (ValueError, UnicodeError):
        response = {
            "api_version": API_VERSION,
            "request_id": "",
            "status": "error",
            "error_code": "invalid_json",
        }
    except PolicyError as error:
        response = {
            "api_version": API_VERSION,
            "request_id": "",
            "status": "error",
            "error_code": str(error),
        }
    print(json.dumps(response, ensure_ascii=False, separators=(",", ":")))


if __name__ == "__main__":
    main()
