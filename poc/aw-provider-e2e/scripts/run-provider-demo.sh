#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly POC_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
readonly REPO_ROOT="$(cd "$POC_ROOT/../.." && pwd)"
readonly PROVIDER_DIR="${AW_PROVIDER_DIR:-$REPO_ROOT/providers}"
readonly FIXTURE="${AW_DEMO_FIXTURE:-$POC_ROOT/fixtures/post-tool-use-builds.json}"
readonly COMMAND_FIXTURE="${AW_COMMAND_FIXTURE:-$POC_ROOT/fixtures/pre-tool-use-command.json}"
readonly HOST_BIN="${AW_PROVIDER_HOST_BIN:-$REPO_ROOT/src/aw/target/debug/aw-provider-host}"
readonly HOOK_BIN="${AW_COSH_HOOK_BIN:-$REPO_ROOT/src/aw/target/debug/aw-cosh-hook}"
readonly LEDGER_BIN="${AW_LEDGER_BIN:-$REPO_ROOT/src/aw/target/debug/aw-ledger}"
readonly TOKENLESS_BIN="${AW_TOKENLESS_BIN:-$REPO_ROOT/src/tokenless/target/debug/tokenless}"
readonly OUTPUT_ROOT="${AW_DEMO_OUTPUT_ROOT:-$REPO_ROOT/target/aw-provider-e2e}"
readonly ROOTS_VALUE="${AW_EXECUTABLE_ROOTS:-$REPO_ROOT/src/tokenless/target/debug:$REPO_ROOT/src/agent-sec-core/agent-sec-cli/.venv/bin}"

die() {
    printf 'aw-provider-demo: ERROR: %s\n' "$*" >&2
    exit 1
}

for file in \
    "$FIXTURE" \
    "$COMMAND_FIXTURE" \
    "$HOST_BIN" \
    "$HOOK_BIN" \
    "$LEDGER_BIN" \
    "$TOKENLESS_BIN"; do
    [[ -f "$file" && ! -L "$file" ]] || die "missing regular file: $file"
done
for binary in "$HOST_BIN" "$HOOK_BIN" "$LEDGER_BIN" "$TOKENLESS_BIN"; do
    [[ -x "$binary" ]] || die "binary is not executable: $binary"
done
[[ -d "$PROVIDER_DIR" && ! -L "$PROVIDER_DIR" ]] ||
    die "provider directory is missing or unsafe: $PROVIDER_DIR"

IFS=: read -r -a executable_roots <<<"$ROOTS_VALUE"
host_root_args=()
hook_root_args=()
for root in "${executable_roots[@]}"; do
    [[ -n "$root" && -d "$root" && ! -L "$root" ]] ||
        die "executable root is missing or unsafe: $root"
    host_root_args+=(--executable-root "$root")
    hook_root_args+=(--executable-root "$root")
done

mkdir -p "$OUTPUT_ROOT"
run_dir="$(mktemp -d "$OUTPUT_ROOT/run.XXXXXXXX")"
readonly run_dir
readonly ledger_dir="$run_dir/ledger"
readonly doctor_json="$run_dir/provider-doctor.jsonl"
readonly hook_json="$run_dir/hook-response.json"
readonly receipts_json="$run_dir/receipts.jsonl"
readonly command_json="$run_dir/command-response.json"
readonly command_receipts_json="$run_dir/command-receipts.jsonl"
readonly native_text_request="$run_dir/tokenless-native-text-request.json"
readonly native_text_response="$run_dir/tokenless-native-text-response.json"
readonly native_text_stderr="$run_dir/tokenless-native-text.stderr"
readonly native_structured_request="$run_dir/tokenless-native-structured-request.json"
readonly native_structured_response="$run_dir/tokenless-native-structured-response.json"
readonly native_structured_stderr="$run_dir/tokenless-native-structured.stderr"

timeout --signal=TERM --kill-after=2s 30s \
    "$HOST_BIN" --output jsonl doctor \
    --manifest-dir "$PROVIDER_DIR" \
    "${host_root_args[@]}" >"$doctor_json"

timeout --signal=TERM --kill-after=2s 15s python3 - \
    "$FIXTURE" "$native_text_request" "$native_structured_request" <<'PY'
import json
import pathlib
import sys

fixture = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
content = fixture["tool_response"]["llmContent"]
json.loads(content)
scope = fixture["execution_scope"]
base = {
    "protocol_version": 1,
    "content": content,
    "input_media_type": "application/json",
    "agent_id": "aw-provider",
    "session_id": scope["agent_session_id"],
    "tool_use_id": scope["tool_use_id"],
    "tool_name": fixture["tool_name"],
    "seam": "post_tool",
    "content_origin": "api_response",
    "capabilities": {
        "replace_output": True,
        "publish_retrieve_tool": False,
        "replace_with_text": True,
    },
}
for path, replace_with_text in [
    (pathlib.Path(sys.argv[2]), True),
    (pathlib.Path(sys.argv[3]), False),
]:
    request = {**base, "capabilities": {**base["capabilities"]}}
    request["capabilities"]["replace_with_text"] = replace_with_text
    path.write_text(json.dumps(request, separators=(",", ":")) + "\n", encoding="utf-8")
PY

TOKENLESS_COMPRESSION_ENABLED=1 \
TOKENLESS_STATS_ENABLED=0 \
TOKENLESS_SLS_ENABLED=0 \
    timeout --signal=TERM --kill-after=5s 60s \
    "$TOKENLESS_BIN" compress \
    <"$native_text_request" >"$native_text_response" 2>"$native_text_stderr"
TOKENLESS_COMPRESSION_ENABLED=1 \
TOKENLESS_STATS_ENABLED=0 \
TOKENLESS_SLS_ENABLED=0 \
    timeout --signal=TERM --kill-after=5s 60s \
    "$TOKENLESS_BIN" compress \
    <"$native_structured_request" \
    >"$native_structured_response" 2>"$native_structured_stderr"

timeout --signal=TERM --kill-after=5s 60s \
    "$HOOK_BIN" \
    --event PreToolUse \
    --manifest-dir "$PROVIDER_DIR" \
    "${hook_root_args[@]}" \
    --target-id ubuntu-vm-poc \
    --provider-wall-time-ms 5000 \
    --allow-unenforced-provider \
    --ledger "$ledger_dir" \
    --ledger-mode required \
    --receipt-log "$command_receipts_json" \
    <"$COMMAND_FIXTURE" >"$command_json"

timeout --signal=TERM --kill-after=5s 60s \
    "$HOOK_BIN" \
    --event PostToolUse \
    --manifest-dir "$PROVIDER_DIR" \
    "${hook_root_args[@]}" \
    --target-id ubuntu-vm-poc \
    --provider-wall-time-ms 5000 \
    --allow-unenforced-provider \
    --ledger "$ledger_dir" \
    --ledger-mode required \
    --receipt-log "$receipts_json" \
    <"$FIXTURE" >"$hook_json"

timeout --signal=TERM --kill-after=2s 15s \
    "$LEDGER_BIN" --ledger "$ledger_dir" verify >"$run_dir/ledger-verify.txt"
timeout --signal=TERM --kill-after=2s 15s \
    "$LEDGER_BIN" --ledger "$ledger_dir" list >"$run_dir/ledger-list.txt"

timeout --signal=TERM --kill-after=2s 15s \
    python3 "$SCRIPT_DIR/summarize.py" \
    --fixture "$FIXTURE" \
    --command-fixture "$COMMAND_FIXTURE" \
    --doctor "$doctor_json" \
    --hook-response "$hook_json" \
    --receipts "$receipts_json" \
    --command-response "$command_json" \
    --command-receipts "$command_receipts_json" \
    --native-text-request "$native_text_request" \
    --native-text-response "$native_text_response" \
    --native-structured-request "$native_structured_request" \
    --native-structured-response "$native_structured_response" \
    --ledger-verify "$run_dir/ledger-verify.txt" \
    --ledger-list "$run_dir/ledger-list.txt" \
    --output "$run_dir/summary.json"

printf '\nEvidence directory: %s\n' "$run_dir"
