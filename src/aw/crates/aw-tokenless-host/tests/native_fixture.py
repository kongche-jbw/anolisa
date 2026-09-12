"""Synthetic protocol peer for Host transport tests; no compression engine."""

import json
import os
import sys
import time
from pathlib import Path

mode = os.environ["MODE"]
if sys.argv[1:] == ["--version"]:
    print("tokenless 0.8.0" if mode == "wrong_version" else "tokenless 0.8.1")
    raise SystemExit(0)
assert sys.argv[1:] == ["compress"]
request = json.load(sys.stdin)
Path("called.json").write_text(json.dumps({"request": request, "environment": dict(os.environ)}))
if mode == "golden":
    print(Path("response.json").read_text())
    raise SystemExit(0)
if mode == "timeout":
    time.sleep(10)
if mode == "failure":
    print("private native diagnostic", file=sys.stderr)
    raise SystemExit(7)
if mode == "pin_drift":
    Path(__file__).write_text("modified during invocation")
source = request["input"]["content"]


# Fixtures use ASCII plus Han characters, matching the official estimator here.
def tokens(text: str) -> int:
    cjk = sum("\u4e00" <= c <= "\u9fff" for c in text)
    return cjk + (len(text) - cjk + 3) // 4


result = {
    "output": "done\n",
    "disposition": "applied",
    "content_type": "build_log",
    "applied_operations": ["terminal_cleanup"],
    "recoverability": "lossless",
    "before_tokens": tokens(source),
    "after_tokens": tokens("done\n"),
    "stash_keys": [],
    "tokenizer_id": "heuristic-v1",
}
if mode in ("passthrough", "no_savings", "dry_run", "recoverability_unavailable"):
    result.update(
        output=source, disposition=mode, applied_operations=[], after_tokens=tokens(source)
    )
if mode == "bad_preserve":
    result.update(disposition="passthrough", applied_operations=[])
if mode == "attribution":
    request["attribution"]["tool_use_id"] = "wrong-tool"
if mode in ("retrievable", "unrecoverable"):
    result["recoverability"] = mode
if mode == "stash":
    result["stash_keys"] = ["stash"]
if mode.startswith("operation_"):
    result["applied_operations"] = [mode.removeprefix("operation_")]
if mode == "operations":
    result["applied_operations"] = []
if mode == "no_reduction":
    result.update(output=source, after_tokens=tokens(source))
if mode == "measurement":
    result["before_tokens"] += 1
if mode == "unknown_tokenizer":
    result["tokenizer_id"] = "made-up"
if mode == "tool_error":
    result.update(disposition="tool_error", additional_context="private diagnostic")
response = {
    "protocol_version": 2,
    "operation": "post_tool",
    "attribution": request["attribution"],
    "result": result,
}
if mode == "version":
    response["protocol_version"] = 3
if mode == "operation":
    response["operation"] = "pre_tool"
wire = json.dumps(response)
if mode == "duplicate":
    wire = wire.replace('"output":', '"output": "duplicate", "output":', 1)
if mode == "unknown":
    result["original_content"] = source
    wire = json.dumps(response)
print(wire)
