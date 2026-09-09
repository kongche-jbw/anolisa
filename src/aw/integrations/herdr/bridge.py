#!/usr/bin/env python3
"""Publish verified current-session AW summaries through native Herdr metadata."""

import argparse
import json
from pathlib import Path
import socket
import subprocess
import sys
import time

SOURCE = "anolisa.aw"
TOKEN_NAMES = ("aw", "aw_sec", "aw_tokenless", "aw_usage")
MAX_REPLY = 1024 * 1024


def rpc(socket_path: Path, method: str, params: dict) -> dict:
    request = {"id": "aw-bridge", "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(3)
        connection.connect(str(socket_path))
        connection.sendall(json.dumps(request).encode() + b"\n")
        reply = bytearray()
        while b"\n" not in reply:
            chunk = connection.recv(65536)
            if not chunk:
                raise ValueError("Herdr closed its API reply")
            reply.extend(chunk)
            if len(reply) > MAX_REPLY:
                raise ValueError("Herdr reply exceeds the limit")
    response = json.loads(bytes(reply).split(b"\n", 1)[0])
    if response.get("id") != request["id"] or response.get("error"):
        raise ValueError("Herdr rejected metadata request")
    result = response["result"]
    envelope = {"pane.get": "pane", "pane.process_info": "process_info"}.get(method)
    return result[envelope] if envelope else result


def verify_pane(binding: dict, pane: dict, process: dict) -> None:
    session = pane.get("agent_session")
    if not session or session.get("value") != binding["scope"]["session_id"]:
        raise ValueError("native session is not bound to this pane")
    pids = [process.get("shell_pid")]
    pids.extend(item["pid"] for item in process.get("foreground_processes", []))
    if binding["agent_pid"] not in pids:
        raise ValueError("runtime process is not in this pane")


def number(record: dict, key: str) -> int:
    value = record[key]
    if type(value) is not int or value < 0:
        raise ValueError("invalid verified counter")
    return value


def format_view(view: dict, scope: dict) -> dict:
    if (
        view.get("format") != 1
        or view.get("scope") != scope
        or view.get("verification") != "journal_verified"
        or view.get("runtime_alive") is not True
        or view.get("adoption") not in ("not_observed", "local_history")
    ):
        raise ValueError("view is not a verified current runtime")
    rows = {"aw_sec": "SecCore: no calls", "aw_tokenless": "Tokenless: no calls"}
    calls = failed = bypasses = 0
    seen = set()
    for provider in view["providers"]:
        identity = provider["provider_id"]
        if identity in seen:
            raise ValueError("duplicate provider in view")
        seen.add(identity)
        count = number(provider, "calls")
        candidates = number(provider, "candidates")
        failures = number(provider, "failed")
        bypassed = number(provider, "bypassed")
        adopted = number(provider, "adopted")
        saved = number(provider, "saved_bytes")
        if (
            adopted > candidates
            or (saved and not adopted)
            or (view["adoption"] == "not_observed" and (adopted or saved))
            or candidates > count
            or failures + bypassed > count
        ):
            raise ValueError("unsupported adoption or inconsistent counters")
        calls += count
        failed += failures
        bypasses += bypassed
        # The verifier's provider identities are configured, never inferred from labels.
        kind = provider.get("kind")
        if kind == "security" or identity == "sec-core":
            token, label = "aw_sec", "SecCore"
        elif kind == "projection" or identity == "tokenless":
            token, label = "aw_tokenless", "Tokenless"
        else:
            raise ValueError("provider lacks a supported display kind")
        if token in seen:
            raise ValueError("multiple providers of one display kind")
        seen.add(token)
        detail = f"{count} calls"
        if adopted:
            detail = f"{adopted} history adopted / -{saved} B"
        elif candidates:
            detail += f" / {candidates} candidates"
        if failures:
            detail += f" / {failures} failed"
        if bypassed:
            detail += f" / {bypassed} bypassed"
        rows[token] = f"{label}: {detail}"
    outcome = []
    if failed:
        outcome.append(f"{failed} failed")
    if bypasses:
        outcome.append(f"{bypasses} bypassed")
    usage = (
        "History evidence"
        if view["adoption"] == "local_history"
        else "Adoption unknown"
    )
    # Keep mixed outcomes visible even when a long Provider row is clipped.
    usage += " | " + (" / ".join(outcome) if outcome else "Ctrl+B Q: detach")
    return {
        "aw": f"AW {'failure' if failed else 'verified'} / {calls} calls",
        **rows,
        "aw_usage": usage,
    }


def publish(
    socket_path: Path, pane_id: str, tokens: dict, seq: int, ttl_ms: int = 5000
) -> None:
    rpc(
        socket_path,
        "pane.report_metadata",
        {
            "pane_id": pane_id,
            "source": SOURCE,
            "seq": seq,
            "ttl_ms": ttl_ms,
            "tokens": tokens,
        },
    )


def refresh(
    socket_path: Path, pane_id: str, verifier: Path, binding_path: Path, seq: int
) -> bool:
    try:
        binding = json.loads(binding_path.read_text())
        pane = rpc(socket_path, "pane.get", {"pane_id": pane_id})
        process = rpc(socket_path, "pane.process_info", {"pane_id": pane_id})
        verify_pane(binding, pane, process)
        result = subprocess.run(
            [str(verifier), str(binding_path)],
            check=True,
            capture_output=True,
            timeout=5,
        )
        if len(result.stdout) > MAX_REPLY:
            raise ValueError("verifier output exceeds the limit")
        tokens = format_view(json.loads(result.stdout), binding["scope"])
        # Reject a session switch during verification before publishing old counters.
        verify_pane(
            binding,
            rpc(socket_path, "pane.get", {"pane_id": pane_id}),
            rpc(socket_path, "pane.process_info", {"pane_id": pane_id}),
        )
        publish(socket_path, pane_id, tokens, seq)
        return True
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        publish(
            socket_path,
            pane_id,
            {
                "aw": "AW unknown / binding or evidence unavailable",
                "aw_sec": None,
                "aw_tokenless": None,
                "aw_usage": None,
            },
            seq,
        )
        return False


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", required=True, type=Path)
    parser.add_argument("--pane", required=True)
    parser.add_argument("--verifier", required=True, type=Path)
    parser.add_argument("--binding", required=True, type=Path)
    parser.add_argument(
        "--duration",
        type=int,
        default=0,
        help="0 publishes once; 1..3600 watches for bounded seconds",
    )
    args = parser.parse_args()
    if not 0 <= args.duration <= 3600:
        parser.error("duration must be in 0..3600 seconds")
    deadline = time.monotonic() + args.duration
    seq = time.time_ns()
    success = True
    try:
        while True:
            success = refresh(args.socket, args.pane, args.verifier, args.binding, seq)
            seq += 1
            if time.monotonic() >= deadline:
                break
            time.sleep(max(0, min(1, deadline - time.monotonic())))
    finally:
        if args.duration:
            publish(args.socket, args.pane, {key: None for key in TOKEN_NAMES}, seq + 1)
    if not success:
        sys.exit("AW summary unavailable; previous counters cleared")


if __name__ == "__main__":
    main()
