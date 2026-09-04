#!/usr/bin/env python3
"""Summarize one real AW multi-Provider run without hiding its identities."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any


EXPECTED_SOURCE_DIGEST = "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1"
EXPECTED_CANDIDATE_DIGEST = "6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003"
EXPECTED_COMMAND_DIGEST = "e3086deb53bcbd1e005b6f708c9b902c2f6a76fc51162dc36b82834605beaf9b"


def load_json(path: Path) -> dict[str, Any]:
    """Load one JSON object from a regular file."""
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"expected an object in {path}")
    return value


def load_first_json_line(path: Path) -> dict[str, Any]:
    """Load the first non-empty JSON object from a JSONL file."""
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.strip():
            value = json.loads(line)
            if not isinstance(value, dict):
                raise ValueError(f"expected an object in {path}")
            return value
    raise ValueError(f"no JSON record in {path}")


def meter(receipt: dict[str, Any], meter_id: str) -> int | None:
    """Read one numeric meter from a Provider receipt."""
    for item in receipt.get("meters", []):
        if item.get("meter_id") == meter_id:
            value = item.get("value")
            return value if isinstance(value, int) else None
    return None


def require_complete_scan(receipt: dict[str, Any], expected_bytes: int) -> None:
    """Reject a demo claim unless one Provider proved complete byte coverage."""
    if receipt.get("disposition") != "produced":
        raise ValueError("security Provider did not produce a usable fact")
    scanned_bytes = meter(receipt, "security.scanned_bytes")
    if scanned_bytes != expected_bytes:
        raise ValueError(
            f"security Provider covered {scanned_bytes!r} bytes; expected {expected_bytes}"
        )
    findings = meter(receipt, "security.findings_total")
    if findings is None or findings < 0:
        raise ValueError("security Provider did not report a valid findings total")


def short_digest(value: str | None) -> str:
    """Format a digest for the terminal while JSON keeps the full value."""
    if not value:
        return "n/a"
    return f"{value[:12]}…{value[-8:]}"


def main() -> None:
    """Build and print a stable, beginner-readable execution summary."""
    parser = argparse.ArgumentParser()
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--command-fixture", type=Path, required=True)
    parser.add_argument("--doctor", type=Path, required=True)
    parser.add_argument("--hook-response", type=Path, required=True)
    parser.add_argument("--receipts", type=Path, required=True)
    parser.add_argument("--command-response", type=Path, required=True)
    parser.add_argument("--command-receipts", type=Path, required=True)
    parser.add_argument("--native-text-request", type=Path, required=True)
    parser.add_argument("--native-text-response", type=Path, required=True)
    parser.add_argument("--native-structured-request", type=Path, required=True)
    parser.add_argument("--native-structured-response", type=Path, required=True)
    parser.add_argument("--ledger-verify", type=Path, required=True)
    parser.add_argument("--ledger-list", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    fixture = load_json(args.fixture)
    command_fixture = load_json(args.command_fixture)
    doctor = load_first_json_line(args.doctor)
    hook = load_json(args.hook_response)
    run = load_first_json_line(args.receipts)
    command_hook = load_json(args.command_response)
    command_run = load_first_json_line(args.command_receipts)
    native_text_request = load_json(args.native_text_request)
    native_text_response = load_json(args.native_text_response)
    native_structured_request = load_json(args.native_structured_request)
    native_structured_response = load_json(args.native_structured_response)
    source = fixture["tool_response"]["llmContent"]
    source_bytes = source.encode("utf-8")
    source_digest = hashlib.sha256(source_bytes).hexdigest()
    if len(source_bytes) != 693 or source_digest != EXPECTED_SOURCE_DIGEST:
        raise ValueError("Provider example source bytes do not match the documented fixture")
    replacement = hook.get("hookSpecificOutput", {}).get("updatedToolResponse")
    if not isinstance(replacement, str) or not replacement:
        raise ValueError("the real Tokenless run did not offer a replacement")
    replacement_digest = hashlib.sha256(replacement.encode("utf-8")).hexdigest()
    if replacement_digest != EXPECTED_CANDIDATE_DIGEST:
        raise ValueError("Provider candidate digest does not match the documented fixture")
    command = command_fixture["tool_input"]["command"]
    command_digest = hashlib.sha256(command.encode("utf-8")).hexdigest()

    for request, replace_with_text in [
        (native_text_request, True),
        (native_structured_request, False),
    ]:
        if request.get("content") != source:
            raise ValueError("native Tokenless replay did not receive the canonical source bytes")
        if request.get("input_media_type") != "application/json":
            raise ValueError("native Tokenless request did not retain application/json")
        if (
            request.get("capabilities", {}).get("replace_with_text")
            is not replace_with_text
        ):
            raise ValueError("native Tokenless request used the wrong representation capability")
    if native_text_response.get("disposition") != "applied":
        raise ValueError("Tokenless text replay did not apply its projection")
    if native_text_response.get("output_media_type") != "text/plain":
        raise ValueError("Tokenless text replay did not declare text/plain")
    if native_text_response.get("output") != replacement:
        raise ValueError("native Tokenless text replay differs from the Provider Host candidate")
    if (
        native_text_response.get("before_tokens") != 174
        or native_text_response.get("after_tokens") != 110
        or native_text_response.get("reversibility") != "lossless"
        or len(replacement.encode("utf-8")) != 438
    ):
        raise ValueError("Tokenless text replay did not produce the stable lossless fixture")
    if native_structured_response.get("output_media_type") != "application/json":
        raise ValueError("Tokenless structured replay did not preserve application/json")
    if (
        native_structured_response.get("disposition") != "no_savings"
        or native_structured_response.get("output") != source
        or native_structured_response.get("before_tokens") != 174
        or native_structured_response.get("after_tokens") != 174
    ):
        raise ValueError("Tokenless structured replay did not retain the stable JSON fixture")
    try:
        json.loads(native_structured_response["output"])
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise ValueError("Tokenless structured replay returned non-JSON bytes") from error

    capabilities = doctor["graph"]["capabilities"]
    receipts = run.get("receipts", [])
    content_receipt = next(
        item
        for item in receipts
        if item["capability"]["id"] == "security.content.inspect"
    )
    code_receipt = next(
        item
        for item in receipts
        if item["capability"]["id"] == "security.code.inspect"
    )
    tokenless_receipt = next(
        item
        for item in receipts
        if item["capability"]["id"] == "context.projection.prepare"
    )
    command_receipt = next(
        item
        for item in command_run.get("receipts", [])
        if item["capability"]["id"] == "security.command.inspect"
    )
    command_bytes = len(command.encode("utf-8"))
    if command_bytes != 38 or command_digest != EXPECTED_COMMAND_DIGEST:
        raise ValueError("command example bytes do not match the documented fixture")
    for receipt, expected_bytes in [
        (command_receipt, command_bytes),
        (content_receipt, len(source_bytes)),
        (code_receipt, len(source_bytes)),
    ]:
        require_complete_scan(receipt, expected_bytes)
    if (
        meter(command_receipt, "security.findings_total") != 1
        or command_run.get("gate") != "warn"
        or command_hook.get("decision") != "allow"
    ):
        raise ValueError("command fixture did not settle as one warning and an allow adapter reply")
    if tokenless_receipt.get("disposition") != "produced":
        raise ValueError("Tokenless did not produce a projection candidate")
    if (
        meter(tokenless_receipt, "context.source_tokens")
        != native_text_response["before_tokens"]
        or meter(tokenless_receipt, "context.prepared_tokens")
        != native_text_response["after_tokens"]
    ):
        raise ValueError("Provider receipt meters differ from the native Tokenless response")

    summary = {
        "schema": "aw.provider.vm-demo-summary/v1",
        "discovery": {
            "status": doctor["status"],
            "provider_ids": sorted({item["provider_id"] for item in capabilities}),
            "capability_count": len(capabilities),
            "guarantees": sorted({item["guarantee"] for item in capabilities}),
        },
        "input": {
            "tool_name": fixture["tool_name"],
            "tool_use_id": fixture["execution_scope"]["tool_use_id"],
            "media_type": native_text_request["input_media_type"],
            "source_bytes": len(source_bytes),
            "source_digest": source_digest,
        },
        "command_check": {
            "tool_name": command_fixture["tool_name"],
            "command": command,
            "command_bytes": command_bytes,
            "command_digest": command_digest,
            "provider_id": command_receipt["provider_id"],
            "provider_disposition": command_receipt["disposition"],
            "input_digest": command_receipt["input_digest"],
            "scanned_bytes": meter(command_receipt, "security.scanned_bytes"),
            "findings": meter(command_receipt, "security.findings_total"),
            "core_gate": command_run.get("gate"),
            "adapter_decision": command_hook.get("decision"),
            "operator_message": command_hook.get("systemMessage"),
            "executed_bytes_proven": False,
        },
        "observations": [
            {
                "capability": content_receipt["capability"],
                "provider_id": content_receipt["provider_id"],
                "disposition": content_receipt["disposition"],
                "scanned_bytes": meter(content_receipt, "security.scanned_bytes"),
                "findings": meter(content_receipt, "security.findings_total"),
                "input_digest": content_receipt["input_digest"],
            },
            {
                "capability": code_receipt["capability"],
                "provider_id": code_receipt["provider_id"],
                "disposition": code_receipt["disposition"],
                "scanned_bytes": meter(code_receipt, "security.scanned_bytes"),
                "findings": meter(code_receipt, "security.findings_total"),
                "input_digest": code_receipt["input_digest"],
            },
        ],
        "projection": {
            "provider_id": tokenless_receipt["provider_id"],
            "disposition": tokenless_receipt["disposition"],
            "before_tokens": meter(tokenless_receipt, "context.source_tokens"),
            "after_tokens": meter(tokenless_receipt, "context.prepared_tokens"),
            "meter_method": next(
                item.get("method")
                for item in tokenless_receipt["meters"]
                if item["meter_id"] == "context.source_tokens"
            ),
            "replacement_bytes": len(replacement.encode("utf-8")),
            "replacement_digest": replacement_digest,
            "media_type": native_text_response["output_media_type"],
            "replacement": replacement,
        },
        "native_protocol_replay": {
            "text_reencoding": {
                "input_media_type": native_text_request["input_media_type"],
                "replace_with_text": True,
                "output_media_type": native_text_response["output_media_type"],
                "response_matches_provider_candidate": True,
            },
            "structured_control": {
                "input_media_type": native_structured_request["input_media_type"],
                "replace_with_text": False,
                "output_media_type": native_structured_response["output_media_type"],
                "disposition": native_structured_response["disposition"],
                "output_bytes": len(native_structured_response["output"].encode("utf-8")),
                "output_is_json": True,
            },
        },
        "ledger": {
            "records_written": [command_run.get("ledger"), run.get("ledger")],
            "verification": args.ledger_verify.read_text(encoding="utf-8").strip(),
            "records": args.ledger_list.read_text(encoding="utf-8").strip(),
            "content_persisted": False,
        },
        "boundary": {
            "replacement_requested": bool(run.get("replacement_requested")),
            "final_adoption_proven": False,
            "note": (
                "This standalone adapter proves candidate production only; "
                "COSH final adoption uses the first-class effective-bytes boundary. "
                f"The command check proves the submitted {command_bytes} bytes were "
                "scanned, not that the same bytes were later executed."
            ),
        },
    }
    temporary_output = args.output.with_name(f".{args.output.name}.tmp")
    temporary_output.write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    temporary_output.replace(args.output)

    print("AW Provider VM demonstration")
    print("=" * 72)
    print(
        f"1  DISCOVER  {doctor['status']} · "
        f"{len(summary['discovery']['provider_ids'])} providers · "
        f"{len(capabilities)} capabilities"
    )
    print(
        f"2  INPUT     tool={summary['input']['tool_name']} · "
        f"bytes={len(source_bytes)} · sha256={short_digest(source_digest)}"
    )
    print(
        "3  MEDIATE   agent-sec-core command · "
        f"gate={summary['command_check']['core_gate']} · "
        f"findings={summary['command_check']['findings']} · "
        f"sha256={short_digest(command_digest)}"
    )
    print(
        "4  OBSERVE   agent-sec-core content+code · "
        f"findings={sum(item['findings'] or 0 for item in summary['observations'])} · "
        f"scanned={sum(item['scanned_bytes'] or 0 for item in summary['observations'])} bytes"
    )
    print(
        "5  ADVISE    tokenless · "
        f"{summary['projection']['before_tokens']}→"
        f"{summary['projection']['after_tokens']} tokens · "
        f"sha256={short_digest(replacement_digest)}"
    )
    print(
        "   MEDIA     application/json→text/plain · "
        "replace_with_text=false keeps application/json"
    )
    print(
        "6  LEDGER    content-free gate + plan records · "
        f"plan={run.get('ledger', {}).get('event_id', 'n/a')}"
    )
    print("7  ADOPTION  requested by adapter; final COSH proof is not claimed here")
    print("-" * 72)
    print(replacement)


if __name__ == "__main__":
    main()
