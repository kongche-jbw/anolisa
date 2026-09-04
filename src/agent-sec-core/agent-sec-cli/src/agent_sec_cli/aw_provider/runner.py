"""Bounded stdin-to-stdout runner for the AW Provider entrypoint.

Every protocol-level outcome exits zero. Invalid wire input raises a protocol
error so the Host records a process failure rather than accepting an untyped
response.
"""

import json
from typing import BinaryIO, TextIO

from pydantic import ValidationError

from agent_sec_cli.aw_provider.handlers import handle
from agent_sec_cli.aw_provider.protocol import PROVIDER_REQUEST_ADAPTER

MAX_REQUEST_BYTES = 64 * 1024 * 1024


class ProviderProtocolError(Exception):
    """Raised when standard input is not one usable native request."""


def _read_bounded_bytes(stdin: BinaryIO | TextIO) -> bytes:
    """Reads at most one byte beyond the native request limit.

    The CLI passes ``sys.stdin``, whose binary buffer is selected here. Text
    streams remain supported for direct library callers, but production input
    is always bounded before UTF-8 decoding.
    """
    stream = getattr(stdin, "buffer", stdin)
    raw = stream.read(MAX_REQUEST_BYTES + 1)
    if isinstance(raw, str):
        raw = raw.encode("utf-8")
    if len(raw) > MAX_REQUEST_BYTES:
        raise ProviderProtocolError(
            f"request exceeds the {MAX_REQUEST_BYTES}-byte limit"
        )
    return raw


def run_provider(stdin: BinaryIO | TextIO, stdout: TextIO) -> None:
    """Reads one native request and writes one native response.

    Raises:
        ProviderProtocolError: Input is oversized, not strict UTF-8 or JSON, or
            does not satisfy the operation-specific request model.
    """
    encoded = _read_bounded_bytes(stdin)
    try:
        raw = encoded.decode("utf-8", errors="strict")
    except UnicodeDecodeError as exc:
        raise ProviderProtocolError("request is not valid UTF-8") from exc

    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ProviderProtocolError(f"request is not valid JSON: {exc.msg}") from exc

    try:
        request = PROVIDER_REQUEST_ADAPTER.validate_python(payload)
    except ValidationError as exc:
        raise ProviderProtocolError(
            f"request does not satisfy the native schema: {exc.error_count()} problems"
        ) from exc

    response = handle(request)
    json.dump(response.model_dump(mode="json"), stdout)
    stdout.write("\n")
    stdout.flush()
