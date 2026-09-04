#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly POC_ROOT="$(readlink -f /opt/anolisa-mvp/aw-provider-poc/current)"
case "$POC_ROOT" in
    /opt/anolisa-mvp/aw-provider-poc/releases/*) ;;
    *) printf 'aw-cosh-adoption-demo: unsafe release path: %s\n' "$POC_ROOT" >&2; exit 1 ;;
esac

readonly STATE_ROOT="${XDG_STATE_HOME:-$HOME/.local/state}/aw-provider-poc/adoption"
readonly TEST_BINARY="$POC_ROOT/bin/cosh-final-adoption-test"
readonly REPOSITORY_ROOT="$POC_ROOT/e2e-repository"
readonly LEDGER_BINARY="$POC_ROOT/bin/aw-ledger"
readonly EXACT_TEST='core::tests::real_providers_commit_effective_history_and_adoption_evidence'

die() {
    printf 'aw-cosh-adoption-demo: ERROR: %s\n' "$*" >&2
    exit 1
}

for binary in "$TEST_BINARY" "$LEDGER_BINARY"; do
    [[ -f "$binary" && -x "$binary" && ! -L "$binary" ]] ||
        die "missing executable: $binary"
done
[[ -d "$REPOSITORY_ROOT/providers" && ! -L "$REPOSITORY_ROOT" ]] ||
    die "E2E repository fixture is unavailable"

mkdir -p "$STATE_ROOT"
run_dir="$(mktemp -d "$STATE_ROOT/run.XXXXXXXX")"
readonly run_dir
readonly ledger_root="$run_dir/ledger"
readonly test_output="$run_dir/test-output.txt"
readonly ledger_verify="$run_dir/ledger-verify.txt"
readonly ledger_list="$run_dir/ledger-list.txt"

if ! AW_E2E_REPOSITORY_ROOT="$REPOSITORY_ROOT" \
    AW_E2E_LEDGER_ROOT="$ledger_root" \
    RUST_TEST_THREADS=1 \
    timeout --signal=TERM --kill-after=5s 120s \
    "$TEST_BINARY" \
    --ignored --exact "$EXACT_TEST" --nocapture >"$test_output" 2>&1; then
    sed -n '1,240p' "$test_output" >&2
    die "the exact Cosh final-adoption E2E test failed"
fi

timeout --signal=TERM --kill-after=2s 15s \
    "$LEDGER_BINARY" --ledger "$ledger_root" verify >"$ledger_verify"
timeout --signal=TERM --kill-after=2s 15s \
    "$LEDGER_BINARY" --ledger "$ledger_root" list >"$ledger_list"

timeout --signal=TERM --kill-after=2s 15s python3 - \
    "$test_output" \
    "$ledger_verify" \
    "$ledger_list" \
    "$POC_ROOT/build-info.json" \
    "$run_dir/summary.json" \
    "$ledger_root" \
    "$EXACT_TEST" <<'PY'
import json
import pathlib
import sys

test_output, verify_path, list_path, build_path, output_path = map(
    pathlib.Path, sys.argv[1:6]
)
expected_ledger_root = pathlib.Path(sys.argv[6]).resolve(strict=True)
exact_test = sys.argv[7]
marker = "AW_E2E_SUMMARY="
matches = [
    line.split(marker, 1)[1]
    for line in test_output.read_text(encoding="utf-8").splitlines()
    if marker in line
]
if len(matches) != 1:
    raise SystemExit(f"expected one {marker} line, observed {len(matches)}")
summary = json.loads(matches[0])
expected = {
    "source_bytes": 693,
    "source_digest": "01202f4b809e4ffee777b3b7a62ca0d5007b99738c0abbc537fd1cc29d7422e1",
    "candidate_bytes": 438,
    "candidate_digest": "6c847696df69b21a2997cf599d6caf2bb5af76f418869c16cf07c0dc7e2d3003",
    "plan_records": 1,
    "adoption_records": 1,
    "decision": "adopted",
}
for key, value in expected.items():
    if summary.get(key) != value:
        raise SystemExit(f"unexpected {key}: {summary.get(key)!r}; expected {value!r}")
reported_ledger_root = pathlib.Path(summary.get("ledger_root", "")).resolve(strict=True)
if reported_ledger_root != expected_ledger_root:
    raise SystemExit("test summary Ledger root is outside this evidence run")

verification = verify_path.read_text(encoding="utf-8").strip()
if verification != "verified 2 record(s); chain intact":
    raise SystemExit(f"Ledger verification did not report two records: {verification!r}")
build = json.loads(build_path.read_text(encoding="utf-8"))
if build.get("cosh_final_adoption_test", {}).get("exact_test") != exact_test:
    raise SystemExit("build evidence does not name the executed exact test")
result = {
    "schema": "aw.provider.cosh-final-adoption-vm-demo/v1",
    "source_commit": build["source_commit"],
    "exact_test": exact_test,
    "test_binary_sha256": build["cosh_final_adoption_test"]["sha256"],
    "source": {
        "bytes": summary["source_bytes"],
        "digest": summary["source_digest"],
    },
    "candidate": {
        "bytes": summary["candidate_bytes"],
        "digest": summary["candidate_digest"],
    },
    "ledger": {
        "root": str(expected_ledger_root),
        "plan_records": summary["plan_records"],
        "adoption_records": summary["adoption_records"],
        "decision": summary["decision"],
        "verification": verification,
        "records": list_path.read_text(encoding="utf-8").strip(),
    },
    "claims": {
        "real_agent_sec": True,
        "real_tokenless": True,
        "cosh_history_candidate_bytes": True,
        "final_adoption_recorded_after_history": True,
        "ledger_contains_model_visible_content": False,
    },
}
temporary_output = output_path.with_name(f".{output_path.name}.tmp")
temporary_output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
temporary_output.replace(output_path)
PY

printf 'COSH FINAL ADOPTION E2E\n'
printf '%s\n' '=============================================================================='
printf 'Evidence      %s\n' "$run_dir"
printf 'Source        693B · sha256=01202f4b809e…d7422e1\n'
printf 'Candidate     438B · sha256=6c847696df69…2d3003\n'
printf 'History       exact candidate bytes committed by CoshCore\n'
printf 'Ledger        plan=1 · context_adoption=1 · decision=adopted\n'
printf 'Verification  %s\n' "$(<"$ledger_verify")"
