#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly INSTALL_ROOT=/opt/anolisa-mvp/aw-provider-poc
readonly SERVICE=cosh-gateway@aw-provider-poc.service
readonly SERVICE_FILE=/etc/systemd/system/cosh-gateway@aw-provider-poc.service
readonly SERVICE_STATE=/var/lib/anolisa-aw-provider-poc
readonly SERVICE_RUNTIME=/run/anolisa-aw-provider-poc
readonly USER_NAME=anolisa
readonly USER_HOME=/home/anolisa
readonly USER_EVIDENCE="$USER_HOME/.local/state/aw-provider-poc"
readonly WORKSPACE=/var/lib/anolisa-agent-work/workspaces/interactive-agent
readonly WS_CKPT=/opt/anolisa-mvp/bin/ws-ckpt
readonly WS_CKPT_SOCKET=/run/ws-ckpt-agent-work/ws-ckpt.sock

purge_evidence=false
purge_checkpoints=false
confirmed=false
inventory_file=
preserved_checkpoint_runs=()
purged_checkpoint_runs=()

cleanup_temporary() {
    if [[ -n "${inventory_file:-}" && \
        "$inventory_file" =~ ^/run/aw-provider-poc-inventory\.[A-Za-z0-9]+$ && \
        -f "$inventory_file" && ! -L "$inventory_file" ]]; then
        unlink -- "$inventory_file"
    fi
}
trap cleanup_temporary EXIT

usage() {
    cat <<'EOF'
Usage: cleanup.sh --yes [--purge-evidence] [--purge-checkpoints]

Always removes only the AW Provider PoC plugin, dedicated Gateway service, and
immutable PoC releases. Runtime evidence and created snapshots are preserved by
default. --purge-evidence also removes the dedicated Gateway database, AW
Ledger, and stateless Provider summaries, but keeps checkpoint summaries beside
preserved snapshots. --purge-checkpoints deletes only checkpoint IDs whose
successful summary exactly matches current inventory. Terminal failed or
cancelled Task evidence is preserved because its side effects are unknown.
EOF
}

die() {
    printf 'cleanup-aw-provider-poc: ERROR: %s\n' "$*" >&2
    exit 1
}

require_service_inactive() {
    if timeout --signal=TERM --kill-after=1s 2s \
        systemctl is-active --quiet "$SERVICE"; then
        die "owned Gateway service is still active"
    else
        local status=$?
        case "$status" in
            3|4) return 0 ;;
            *) die "could not verify that the owned Gateway service stopped" ;;
        esac
    fi
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --yes) confirmed=true ;;
        --purge-evidence) purge_evidence=true ;;
        --purge-checkpoints) purge_checkpoints=true; purge_evidence=true ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done

[[ "$confirmed" == true ]] || die "review the scope, then pass --yes"
[[ "$(id -u)" -eq 0 ]] || die "run through sudo"
if [[ -e "$USER_EVIDENCE" || -L "$USER_EVIDENCE" ]]; then
    [[ -d "$USER_EVIDENCE" && ! -L "$USER_EVIDENCE" ]] ||
        die "refusing unexpected evidence-root type: $USER_EVIDENCE"
fi

if [[ "$purge_evidence" == true && \
    ( -e "$USER_EVIDENCE/checkpoints" || -L "$USER_EVIDENCE/checkpoints" ) ]]; then
    [[ -d "$USER_EVIDENCE/checkpoints" && ! -L "$USER_EVIDENCE/checkpoints" ]] ||
        die "refusing unexpected checkpoint-evidence type: $USER_EVIDENCE/checkpoints"
    checkpoint_validation="$(timeout --signal=TERM --kill-after=2s 15s \
        python3 - "$USER_EVIDENCE/checkpoints" <<'PY_VALIDATE_CHECKPOINTS'
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
run_name = re.compile(r"^run\.[A-Za-z0-9]+$")
task_id = re.compile(
    r"^tsk_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
terminal_failure = {"task_failed", "task_cancelled"}
runs = sorted(path for path in root.iterdir() if path.name.startswith("run."))
if len(runs) > 100:
    raise SystemExit("refusing to inspect more than 100 checkpoint Task runs")
for run in runs:
    if run.is_symlink() or not run.is_dir() or not run_name.fullmatch(run.name):
        raise SystemExit(f"unsafe checkpoint Task evidence: {run}")
    submission = run / "submission.json"
    summary = run / "summary.json"
    events_path = run / "task-events.json"
    if submission.is_symlink() or not submission.is_file():
        raise SystemExit(f"incomplete checkpoint Task at {run}; query its task_id first")
    try:
        submitted = json.loads(submission.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid checkpoint Task evidence at {submission.parent}: {error}")
    submitted_task_id = submitted.get("task_id") if isinstance(submitted, dict) else None
    if not isinstance(submitted_task_id, str) or not task_id.fullmatch(submitted_task_id):
        raise SystemExit(f"invalid checkpoint submission identity at {run}")
    if summary.exists() or summary.is_symlink():
        if summary.is_symlink() or not summary.is_file():
            raise SystemExit(f"unsafe checkpoint summary evidence: {summary}")
        try:
            completed = json.loads(summary.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise SystemExit(f"invalid checkpoint summary at {run}: {error}")
        task = completed.get("task") if isinstance(completed, dict) else None
        if (
            not isinstance(task, dict)
            or completed.get("schema") != "aw.provider.checkpoint-vm-demo/v1"
            or task.get("state") != "succeeded"
            or task.get("task_id") != submitted_task_id
        ):
            raise SystemExit(f"checkpoint Task is not proven successful at {run}")
        continue
    if events_path.is_symlink() or not events_path.is_file():
        raise SystemExit(f"incomplete checkpoint Task at {run}; query its task_id first")
    try:
        events = json.loads(events_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise SystemExit(f"invalid checkpoint Task events at {run}: {error}")
    if not isinstance(events, list) or not events:
        raise SystemExit(f"checkpoint Task has no durable events at {run}")
    kinds = []
    for expected_revision, envelope in enumerate(events, start=1):
        header = envelope.get("header") if isinstance(envelope, dict) else None
        event = envelope.get("event") if isinstance(envelope, dict) else None
        correlation = header.get("correlation") if isinstance(header, dict) else None
        if (
            not isinstance(header, dict)
            or not isinstance(event, dict)
            or not isinstance(correlation, dict)
            or header.get("schema") != "cosh.task.event"
            or header.get("schema_version") != 1
            or envelope.get("task_id") != submitted_task_id
            or envelope.get("revision") != expected_revision
            or correlation.get("task_id") != submitted_task_id
            or not isinstance(event.get("event"), str)
        ):
            raise SystemExit(f"invalid checkpoint Task event sequence at {run}")
        kinds.append(event["event"])
    terminals = [kind for kind in kinds if kind in terminal_failure | {"task_succeeded"}]
    if len(terminals) != 1 or kinds[-1] not in terminal_failure:
        raise SystemExit(f"checkpoint Task is not terminal at {run}: {kinds}")
    print(run.name)
PY_VALIDATE_CHECKPOINTS
)" ||
        die "reconcile incomplete checkpoint Tasks before purging evidence"
    if [[ -n "$checkpoint_validation" ]]; then
        mapfile -t preserved_checkpoint_runs <<<"$checkpoint_validation"
    fi
fi

timeout --signal=TERM --kill-after=2s 15s runuser -u "$USER_NAME" -- env \
    HOME="$USER_HOME" \
    PATH="$USER_HOME/.local/bin:/usr/local/bin:/opt/anolisa-mvp/bin:/usr/bin:/bin" \
    /usr/local/bin/herdr plugin unlink anolisa.aw-provider-poc >/dev/null 2>&1 || true
plugin_list="$(timeout --signal=TERM --kill-after=2s 15s \
    runuser -u "$USER_NAME" -- env \
    HOME="$USER_HOME" \
    PATH="$USER_HOME/.local/bin:/usr/local/bin:/opt/anolisa-mvp/bin:/usr/bin:/bin" \
    /usr/local/bin/herdr plugin list)" || die "could not verify Herdr plugin removal"
if grep -F -- '- anolisa.aw-provider-poc (' <<<"$plugin_list" >/dev/null; then
    die "owned Herdr plugin is still linked"
fi

timeout --signal=TERM --kill-after=2s 30s \
    systemctl disable --now "$SERVICE" >/dev/null 2>&1 || true
require_service_inactive

if [[ -e "$SERVICE_RUNTIME" || -L "$SERVICE_RUNTIME" ]]; then
    [[ -d "$SERVICE_RUNTIME" && ! -L "$SERVICE_RUNTIME" ]] ||
        die "refusing unexpected runtime-root type: $SERVICE_RUNTIME"
    rm -rf -- "$SERVICE_RUNTIME"
fi

if [[ "$purge_checkpoints" == true && \
    ( -e "$USER_EVIDENCE/checkpoints" || -L "$USER_EVIDENCE/checkpoints" ) ]]; then
    [[ -d "$USER_EVIDENCE/checkpoints" && ! -L "$USER_EVIDENCE/checkpoints" ]] ||
        die "refusing unexpected checkpoint-evidence type: $USER_EVIDENCE/checkpoints"
    [[ -f "$WS_CKPT" && -x "$WS_CKPT" && ! -L "$WS_CKPT" ]] ||
        die "ws-ckpt executable is unavailable: $WS_CKPT"
    [[ -S "$WS_CKPT_SOCKET" ]] ||
        die "ws-ckpt socket is unavailable: $WS_CKPT_SOCKET"
    inventory_file="$(mktemp /run/aw-provider-poc-inventory.XXXXXXXX)"
    chmod 0600 "$inventory_file"
    timeout --signal=TERM --kill-after=2s 30s \
        runuser -u "$USER_NAME" -- env WS_CKPT_SOCKET="$WS_CKPT_SOCKET" \
        "$WS_CKPT" list \
        --workspace "$WORKSPACE" \
        --format json >"$inventory_file" ||
        die "could not read the current checkpoint inventory"
    checkpoint_output="$(timeout --signal=TERM --kill-after=2s 15s \
        python3 - "$USER_EVIDENCE/checkpoints" "$inventory_file" "$WORKSPACE" <<'PY_PLAN_CHECKPOINTS'
import json
import pathlib
import re
import sys
from datetime import datetime

root = pathlib.Path(sys.argv[1])
inventory_path = pathlib.Path(sys.argv[2])
workspace = sys.argv[3]
pattern = re.compile(
    r"^ckp_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"
)
try:
    inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid current checkpoint inventory: {error}")
if not isinstance(inventory, list):
    raise SystemExit("current checkpoint inventory is not a list")
current = {}
for item in inventory:
    if (
        not isinstance(item, dict)
        or not isinstance(item.get("id"), str)
        or not isinstance(item.get("meta"), dict)
        or item.get("workspace") != workspace
    ):
        raise SystemExit("current checkpoint inventory contains an invalid entry")
    if item["id"] in current:
        raise SystemExit(f"current checkpoint inventory repeats {item['id']}")
    current[item["id"]] = item
summaries = list(root.glob("run.*/summary.json"))
if len(summaries) > 100:
    raise SystemExit("refusing to purge more than 100 recorded checkpoints")
records = []
for path in summaries:
    if (
        path.is_symlink()
        or path.parent.is_symlink()
        or not path.is_file()
        or re.fullmatch(r"run\.[A-Za-z0-9]+", path.parent.name) is None
    ):
        raise SystemExit(f"refusing unsafe checkpoint summary: {path}")
    value = json.loads(path.read_text(encoding="utf-8"))
    if (
        not isinstance(value, dict)
        or value.get("schema") != "aw.provider.checkpoint-vm-demo/v1"
    ):
        raise SystemExit(f"invalid checkpoint summary schema in {path}")
    checkpoint = value.get("checkpoint")
    if not isinstance(checkpoint, dict):
        raise SystemExit(f"invalid checkpoint object in {path}")
    checkpoint_id = checkpoint.get("id")
    if not isinstance(checkpoint_id, str) or not pattern.fullmatch(checkpoint_id):
        raise SystemExit(f"invalid recorded checkpoint ID in {path}: {checkpoint_id!r}")
    checkpoint_meta = checkpoint.get("meta")
    if not isinstance(checkpoint_meta, dict):
        raise SystemExit(f"invalid checkpoint metadata in {path}")
    if checkpoint.get("workspace") != workspace:
        raise SystemExit(f"checkpoint workspace mismatch in {path}")
    created_at = checkpoint_meta.get("created_at")
    if not isinstance(created_at, str) or not created_at.endswith("Z"):
        raise SystemExit(f"invalid checkpoint creation time in {path}: {created_at!r}")
    try:
        created = datetime.fromisoformat(created_at[:-1] + "+00:00")
    except ValueError as error:
        raise SystemExit(
            f"invalid checkpoint creation time in {path}: {created_at!r} ({error})"
        )
    records.append((created, checkpoint_id, path.parent.name, checkpoint))
seen = set()
for _, checkpoint_id, run_name, checkpoint in sorted(records, reverse=True):
    if checkpoint_id in seen:
        raise SystemExit(f"checkpoint summaries repeat {checkpoint_id}")
    seen.add(checkpoint_id)
    current_checkpoint = current.get(checkpoint_id)
    if current_checkpoint is None:
        continue
    # child_ids changes when a newer snapshot is created. All other persisted
    # identity fields must still match the successful summary exactly.
    recorded_identity = {
        **checkpoint,
        "meta": {key: value for key, value in checkpoint["meta"].items() if key != "child_ids"},
    }
    current_identity = {
        **current_checkpoint,
        "meta": {
            key: value
            for key, value in current_checkpoint.get("meta", {}).items()
            if key != "child_ids"
        },
    }
    if current_identity != recorded_identity:
        raise SystemExit(f"current checkpoint inventory drifted for {checkpoint_id}")
    print(f"{checkpoint_id}\t{run_name}")
PY_PLAN_CHECKPOINTS
)" || die "checkpoint evidence validation failed"
    checkpoint_plan=()
    if [[ -n "$checkpoint_output" ]]; then
        mapfile -t checkpoint_plan <<<"$checkpoint_output"
    fi
    for planned in "${checkpoint_plan[@]}"; do
        checkpoint_id=
        run_name=
        extra=
        IFS=$'\t' read -r checkpoint_id run_name extra <<<"$planned"
        [[ "$checkpoint_id" =~ ^ckp_[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ && \
            "$run_name" =~ ^run\.[A-Za-z0-9]+$ && -z "$extra" ]] ||
            die "checkpoint purge plan contains an unsafe identity"
        evidence_run="$USER_EVIDENCE/checkpoints/$run_name"
        [[ -d "$evidence_run" && ! -L "$evidence_run" ]] ||
            die "checkpoint purge plan names unsafe evidence: $evidence_run"
        timeout --signal=TERM --kill-after=2s 30s \
            runuser -u "$USER_NAME" -- env WS_CKPT_SOCKET="$WS_CKPT_SOCKET" \
            "$WS_CKPT" delete \
            --workspace "$WORKSPACE" \
            --snapshot "$checkpoint_id" \
            --force
        purged_checkpoint_runs+=("$evidence_run")
    done
fi

if [[ -e "$SERVICE_FILE" || -L "$SERVICE_FILE" ]]; then
    [[ -f "$SERVICE_FILE" && ! -L "$SERVICE_FILE" ]] ||
        die "refusing unexpected service-file type: $SERVICE_FILE"
    rm -f -- "$SERVICE_FILE"
fi
timeout --signal=TERM --kill-after=2s 15s systemctl daemon-reload
timeout --signal=TERM --kill-after=2s 15s \
    systemctl reset-failed "$SERVICE" >/dev/null 2>&1 || true

if [[ -e "$INSTALL_ROOT" || -L "$INSTALL_ROOT" ]]; then
    [[ -d "$INSTALL_ROOT" && ! -L "$INSTALL_ROOT" ]] ||
        die "refusing unexpected install-root type: $INSTALL_ROOT"
    rm -rf -- "$INSTALL_ROOT"
fi

if [[ "$purge_evidence" == true ]]; then
    evidence_paths=(
        "$SERVICE_STATE"
        "$USER_EVIDENCE/runs"
        "$USER_EVIDENCE/adoption"
    )
    for path in "${evidence_paths[@]}"; do
        if [[ -e "$path" || -L "$path" ]]; then
            [[ -d "$path" && ! -L "$path" ]] ||
                die "refusing unexpected evidence-root type: $path"
            rm -rf -- "$path"
        fi
    done
    if [[ "$purge_checkpoints" == true ]]; then
        for path in "${purged_checkpoint_runs[@]}"; do
            case "$path" in
                "$USER_EVIDENCE/checkpoints"/run.*) ;;
                *) die "refusing unsafe checkpoint evidence path: $path" ;;
            esac
            [[ -d "$path" && ! -L "$path" ]] ||
                die "checkpoint evidence changed during cleanup: $path"
            rm -rf -- "$path"
        done
        rmdir -- "$USER_EVIDENCE/checkpoints" 2>/dev/null || true
    fi
    if [[ -d "$USER_EVIDENCE" ]]; then
        rmdir -- "$USER_EVIDENCE" 2>/dev/null || true
    fi
fi

[[ ! -e "$SERVICE_FILE" && ! -e "$INSTALL_ROOT" && ! -e "$SERVICE_RUNTIME" ]] ||
    die "owned service, runtime, or install files remain"
require_service_inactive

printf 'Removed the AW Provider PoC plugin, dedicated Gateway, and releases.\n'
if [[ "$purge_evidence" == true ]]; then
    printf 'Removed dedicated Gateway, Ledger, and stateless Provider evidence.\n'
else
    printf 'Preserved evidence at %s and %s.\n' "$SERVICE_STATE" "$USER_EVIDENCE"
fi
if [[ "$purge_checkpoints" == false ]]; then
    printf 'Preserved ws-ckpt snapshots and their PoC ownership summaries.\n'
else
    printf 'Removed %d checkpoint(s) proven by successful summaries and exact inventory.\n' \
        "${#purged_checkpoint_runs[@]}"
    if [[ -d "$USER_EVIDENCE/checkpoints" ]]; then
        printf 'Preserved unmatched or terminal failed/cancelled Task evidence at %s.\n' \
            "$USER_EVIDENCE/checkpoints"
        printf 'No snapshot deletion was inferred from %d failed/cancelled Task run(s).\n' \
            "${#preserved_checkpoint_runs[@]}"
    fi
fi
