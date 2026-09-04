#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
readonly OUTPUT_ROOT="${AW_POC_BUILD_ROOT:-$REPO_ROOT/target/aw-provider-e2e}"
readonly EXACT_ADOPTION_TEST=\
'core::tests::real_providers_commit_effective_history_and_adoption_evidence'

die() {
    printf 'build-aw-provider-poc: ERROR: %s\n' "$*" >&2
    exit 1
}

[[ "$(uname -s)" == Linux && "$(uname -m)" == aarch64 ]] ||
    die "the Ubuntu VM bundle must be built on Linux aarch64"
[[ -z "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=normal)" ]] ||
    die "commit or remove worktree changes before creating an evidence bundle"

readonly source_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
[[ "$source_commit" =~ ^[0-9a-f]{40}$ ]] || die "invalid source commit"

require_source_unchanged() {
    [[ "$(git -C "$REPO_ROOT" rev-parse HEAD)" == "$source_commit" ]] ||
        die "repository HEAD changed while building the evidence bundle"
    [[ -z "$(git -C "$REPO_ROOT" status --porcelain --untracked-files=normal)" ]] ||
        die "worktree changed while building the evidence bundle"
}

timeout --signal=TERM --kill-after=15s 1800s cargo build \
    --manifest-path "$REPO_ROOT/src/aw/Cargo.toml" \
    --target-dir "$REPO_ROOT/src/aw/target" \
    --locked --release \
    -p aw-provider-host -p aw-cosh-hook -p aw-ledger
timeout --signal=TERM --kill-after=15s 1800s cargo build \
    --manifest-path "$REPO_ROOT/src/tokenless/Cargo.toml" \
    --target-dir "$REPO_ROOT/src/tokenless/target" \
    --locked --release -p tokenless-cli
timeout --signal=TERM --kill-after=15s 1800s cargo build \
    --manifest-path "$REPO_ROOT/src/cosh-ng/Cargo.toml" \
    --target-dir "$REPO_ROOT/src/cosh-ng/target" \
    --locked --release -p cosh-core -p cosh-gateway

mkdir -p "$OUTPUT_ROOT"
bundle=
archive_tmp=
checksum_tmp=
test_metadata=
cleanup() {
    if [[ -n "${bundle:-}" && "$bundle" == "$OUTPUT_ROOT"/bundle.* ]]; then
        find "$bundle" -type d -exec chmod u+w {} + 2>/dev/null || true
        rm -rf -- "$bundle"
    fi
    [[ -n "${archive_tmp:-}" && "$archive_tmp" == "$OUTPUT_ROOT"/archive.*.tar.gz ]] &&
        rm -f -- "$archive_tmp"
    [[ -n "${checksum_tmp:-}" && "$checksum_tmp" == "$OUTPUT_ROOT"/checksum.* ]] &&
        rm -f -- "$checksum_tmp"
    [[ -n "${test_metadata:-}" && \
        "$test_metadata" == "$OUTPUT_ROOT"/cosh-test-metadata.*.jsonl ]] &&
        rm -f -- "$test_metadata"
}
trap cleanup EXIT

test_metadata="$(mktemp "$OUTPUT_ROOT/cosh-test-metadata.XXXXXXXX.jsonl")"
timeout --signal=TERM --kill-after=15s 1800s cargo test \
    --manifest-path "$REPO_ROOT/src/cosh-ng/Cargo.toml" \
    --target-dir "$REPO_ROOT/src/cosh-ng/target" \
    --locked -p cosh-core --no-run --message-format=json >"$test_metadata"
cosh_test_executable="$(python3 - "$test_metadata" <<'PY'
import json
import pathlib
import sys

matches = []
for line in pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines():
    try:
        message = json.loads(line)
    except json.JSONDecodeError:
        continue
    if (
        message.get("reason") == "compiler-artifact"
        and message.get("target", {}).get("name") == "cosh-core"
        and "bin" in message.get("target", {}).get("kind", [])
        and message.get("profile", {}).get("test") is True
        and isinstance(message.get("executable"), str)
    ):
        matches.append(message["executable"])

matches = sorted(set(matches))
if len(matches) != 1:
    raise SystemExit(f"expected one cosh-core unit-test executable, found {matches}")
print(matches[0])
PY
)"
[[ -f "$cosh_test_executable" && -x "$cosh_test_executable" ]] ||
    die "cargo did not produce the cosh-core unit-test executable"
timeout --signal=TERM --kill-after=2s 15s "$cosh_test_executable" --list | \
    grep -Fx "$EXACT_ADOPTION_TEST: test" >/dev/null ||
    die "selected cosh-core test executable does not contain the exact E2E test"
require_source_unchanged

bundle="$(mktemp -d "$OUTPUT_ROOT/bundle.XXXXXXXX")"

install -d -m 0755 \
    "$bundle/bin" \
    "$bundle/config" \
    "$bundle/e2e-repository/providers" \
    "$bundle/e2e-repository/src/agent-sec-core/agent-sec-cli/.venv/bin" \
    "$bundle/e2e-repository/src/tokenless/target/debug" \
    "$bundle/fixtures" \
    "$bundle/herdr-plugin" \
    "$bundle/providers" \
    "$bundle/python" \
    "$bundle/scripts"

install -m 0555 "$REPO_ROOT/src/aw/target/release/aw-provider-host" \
    "$bundle/bin/aw-provider-host"
install -m 0555 "$REPO_ROOT/src/aw/target/release/aw-cosh-hook" \
    "$bundle/bin/aw-cosh-hook"
install -m 0555 "$REPO_ROOT/src/aw/target/release/aw-ledger" \
    "$bundle/bin/aw-ledger"
install -m 0555 "$REPO_ROOT/src/tokenless/target/release/tokenless" \
    "$bundle/bin/tokenless"
install -m 0555 "$REPO_ROOT/src/cosh-ng/target/release/cosh-core" \
    "$bundle/bin/cosh-core"
install -m 0555 "$REPO_ROOT/src/cosh-ng/target/release/cosh-gateway" \
    "$bundle/bin/cosh-gateway"
install -m 0555 "$cosh_test_executable" \
    "$bundle/bin/cosh-final-adoption-test"
install -m 0555 "$SCRIPT_DIR/vm/guest/agent-sec-cli" \
    "$bundle/bin/agent-sec-cli"
install -m 0555 "$SCRIPT_DIR/vm/guest/aw-provider-demo" \
    "$bundle/bin/aw-provider-demo"
install -m 0555 "$SCRIPT_DIR/vm/guest/aw-provider-dashboard" \
    "$bundle/bin/aw-provider-dashboard"
install -m 0555 "$SCRIPT_DIR/vm/guest/run-checkpoint-demo.py" \
    "$bundle/bin/aw-checkpoint-demo"
install -m 0555 "$SCRIPT_DIR/vm/guest/run-cosh-adoption-demo.sh" \
    "$bundle/bin/aw-cosh-adoption-demo"
install -m 0555 "$SCRIPT_DIR/vm/guest/run-e2e-demo.sh" \
    "$bundle/bin/aw-provider-e2e"
install -m 0444 "$SCRIPT_DIR/vm/guest/cosh-system.toml" \
    "$bundle/config/cosh-system.toml"
install -m 0444 "$SCRIPT_DIR/vm/guest/cosh-gateway@aw-provider-poc.service" \
    "$bundle/config/cosh-gateway@aw-provider-poc.service"

install -m 0444 "$SCRIPT_DIR/fixtures/post-tool-use-builds.json" \
    "$bundle/fixtures/post-tool-use-builds.json"
install -m 0444 "$SCRIPT_DIR/fixtures/pre-tool-use-command.json" \
    "$bundle/fixtures/pre-tool-use-command.json"
install -m 0444 "$SCRIPT_DIR/vm/herdr-plugin/herdr-plugin.toml" \
    "$bundle/herdr-plugin/herdr-plugin.toml"
install -m 0555 "$SCRIPT_DIR/scripts/run-provider-demo.sh" \
    "$bundle/scripts/run-provider-demo.sh"
install -m 0555 "$SCRIPT_DIR/scripts/summarize.py" \
    "$bundle/scripts/summarize.py"

cp -a "$REPO_ROOT/providers/agent-sec-core" "$bundle/providers/"
cp -a "$REPO_ROOT/providers/tokenless" "$bundle/providers/"
cp -a "$REPO_ROOT/providers/agent-sec-core" "$bundle/e2e-repository/providers/"
cp -a "$REPO_ROOT/providers/tokenless" "$bundle/e2e-repository/providers/"
install -m 0555 "$REPO_ROOT/src/tokenless/target/release/tokenless" \
    "$bundle/e2e-repository/src/tokenless/target/debug/tokenless"
install -m 0555 "$SCRIPT_DIR/vm/guest/agent-sec-cli" \
    "$bundle/e2e-repository/src/agent-sec-core/agent-sec-cli/.venv/bin/agent-sec-cli"
tar \
    --exclude='__pycache__' \
    --exclude='*.pyc' \
    -C "$REPO_ROOT/src/agent-sec-core/agent-sec-cli/src" \
    -cf - agent_sec_cli | tar -C "$bundle/python" -xf -

python3 - "$bundle/build-info.json" "$source_commit" \
    "$bundle/bin/cosh-final-adoption-test" "$EXACT_ADOPTION_TEST" <<'PY'
import hashlib
import json
import pathlib
import sys

test_binary = pathlib.Path(sys.argv[3])
exact_test = sys.argv[4]
evidence = {
    "schema": "aw.provider.vm-bundle/v1",
    "source_commit": sys.argv[2],
    "architecture": "aarch64",
    "providers": ["agent-sec-core", "tokenless"],
    "state_provider": "gateway/workspace-checkpoint-v1",
    "cosh_final_adoption_test": {
        "sha256": hashlib.sha256(test_binary.read_bytes()).hexdigest(),
        "size": test_binary.stat().st_size,
        "exact_test": exact_test,
    },
}
pathlib.Path(sys.argv[1]).write_text(
    json.dumps(evidence, indent=2) + "\n", encoding="utf-8"
)
PY
require_source_unchanged
find "$bundle" -type d -exec chmod 0555 {} +
find "$bundle" -type f ! -path '*/bin/*' ! -path '*/scripts/*' \
    -exec chmod 0444 {} +
chmod 0555 "$bundle/e2e-repository/src/tokenless/target/debug/tokenless"

readonly archive="$OUTPUT_ROOT/aw-provider-poc-$source_commit.tar.gz"
readonly archive_tmp="$(mktemp "$OUTPUT_ROOT/archive.XXXXXXXX.tar.gz")"
readonly checksum_tmp="$(mktemp "$OUTPUT_ROOT/checksum.XXXXXXXX")"
tar -C "$bundle" -czf "$archive_tmp" .
sha256sum "$archive_tmp" | sed "s#  $archive_tmp\$#  $(basename "$archive")#" \
    >"$checksum_tmp"
mv -f "$archive_tmp" "$archive"
mv -f "$checksum_tmp" "$archive.sha256"
chmod 0444 "$archive" "$archive.sha256"

printf '%s\n' "$archive"
