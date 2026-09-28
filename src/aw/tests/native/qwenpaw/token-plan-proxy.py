#!/usr/bin/env python3
"""Bounded test relay keeping the real Token Plan key outside Agent profiles."""

from __future__ import annotations

import argparse
import http.server
import json
import os
from pathlib import Path
import time
import urllib.error
import urllib.request

ENDPOINT = "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1"
LIMIT = 4 * 1024 * 1024


def main(output: Path, key_file: Path) -> None:
    metadata = key_file.stat()
    if metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("Model credential file must be current-user private")
    key = key_file.read_text().strip()
    if not key:
        raise ValueError("Empty model credential")
    calls = 0

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def do_GET(self) -> None:
            # Provider discovery is local; inference is always real upstream.
            value = {
                "object": "list",
                "data": [{"id": "qwen3.7-plus", "object": "model"}],
            }
            payload = json.dumps(value).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_POST(self) -> None:
            nonlocal calls
            if self.path != "/v1/chat/completions" or calls >= 6:
                self.send_error(429, "Fixture request limit or route rejected")
                return
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > LIMIT:
                self.send_error(413, "Fixture request bound exceeded")
                return
            body = self.rfile.read(length)
            request = json.loads(body)
            calls += 1
            tool_messages = [
                message for message in request.get("messages", []) if message.get("role") == "tool"
            ]
            record = {
                "call": calls,
                "model": request.get("model"),
                "tool_result_count": len(tool_messages),
                "tool_result_contains_42": any(
                    "42" in json.dumps(message.get("content")) for message in tool_messages
                ),
                "stream": request.get("stream"),
                "real_upstream": True,
            }
            upstream = urllib.request.Request(
                ENDPOINT + "/chat/completions",
                data=body,
                headers={
                    "Authorization": f"Bearer {key}",
                    "Content-Type": "application/json",
                },
            )
            try:
                with urllib.request.urlopen(upstream, timeout=60) as response:
                    payload = response.read(LIMIT + 1)
                    content_type = response.headers.get("Content-Type", "application/json")
                if len(payload) > LIMIT or key.encode() in payload:
                    raise ValueError("Response outside fixture output contract")
                record["http_status"] = 200
                self.send_response(200)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            except urllib.error.HTTPError as error:
                record["http_status"] = error.code
                self.send_error(502, "Upstream model request failed")
            except (OSError, ValueError):
                record["http_status"] = None
                self.send_error(502, "Upstream model request failed")
            finally:
                with (output / "model-requests.jsonl").open("a") as log:
                    log.write(json.dumps(record) + "\n")

    with http.server.HTTPServer(("127.0.0.1", 0), Handler) as server:
        server.timeout = 1
        (output / "model-server.json").write_text(
            json.dumps({"pid": os.getpid(), "port": server.server_port}) + "\n"
        )
        deadline = time.monotonic() + 240
        while time.monotonic() < deadline:
            server.handle_request()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--key-file", type=Path, required=True)
    args = parser.parse_args()
    main(args.output, args.key_file)
