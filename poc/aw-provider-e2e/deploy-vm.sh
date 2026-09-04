#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly VM_ROOT="${AW_POC_VM_ROOT:-/home/kongche/anolisa/ws/ubuntu-26.04-vm}"
readonly STAGE_PREFIX=/home/ubuntu/aw-provider-poc-stage
readonly HERDR_SESSION=anolisa-agent
readonly HERDR_PLUGIN=anolisa.aw-provider-poc
readonly HERDR_ACTION=run-complete-e2e

die() {
    printf 'deploy-aw-provider-poc: ERROR: %s\n' "$*" >&2
    exit 1
}

verify_shared_services() {
    timeout --signal=TERM --kill-after=2s 10s "$VM_ROOT/ssh.sh" \
        "systemctl is-active --quiet ws-ckpt-agent-work.service cosh-gateway@ubuntu.service" ||
        die "the shared ws-ckpt or COSH Gateway service is not active"
}

parse_action_log_id() {
    python3 - 3<&0 <<'PY'
import json
import os
import re

document = json.load(os.fdopen(3, "r", encoding="utf-8"))
log = document.get("result", {}).get("log", {})
if log.get("plugin_id") != "anolisa.aw-provider-poc":
    raise SystemExit("Herdr invocation returned the wrong plugin identity")
if log.get("action_id") != "run-complete-e2e":
    raise SystemExit("Herdr invocation returned the wrong action identity")
log_id = log.get("log_id")
if not isinstance(log_id, str) or re.fullmatch(r"plugin-log-[0-9]+", log_id) is None:
    raise SystemExit("Herdr invocation omitted a canonical action log ID")
print(log_id)
PY
}

select_action_log() {
    python3 - "$1" 3<&0 <<'PY'
import json
import os
import sys

document = json.load(os.fdopen(3, "r", encoding="utf-8"))
logs = document.get("result", {}).get("logs", [])
matches = [item for item in logs if item.get("log_id") == sys.argv[1]]
if len(matches) != 1:
    raise SystemExit(f"expected one Herdr log {sys.argv[1]}, observed {len(matches)}")
print(json.dumps(matches[0], separators=(",", ":")))
PY
}

action_log_status() {
    python3 - 3<&0 <<'PY'
import json
import os

record = json.load(os.fdopen(3, "r", encoding="utf-8"))
status = record.get("status")
if status not in {"running", "succeeded", "failed"}:
    raise SystemExit(f"unsupported Herdr action status: {status!r}")
print(status)
PY
}

render_action_log() {
    python3 - 3<&0 <<'PY'
import json
import os
import sys

record = json.load(os.fdopen(3, "r", encoding="utf-8"))
sys.stdout.write(record.get("stdout", ""))
sys.stderr.write(record.get("stderr", ""))
PY
}

[[ -x "$VM_ROOT/ssh.sh" ]] ||
    die "VM helpers are missing under $VM_ROOT"

timeout --signal=TERM --kill-after=2s 10s "$VM_ROOT/ssh.sh" true >/dev/null 2>&1 ||
    die "the existing VM is not reachable; this script never starts or stops QEMU"
verify_shared_services

readonly archive="$(timeout --signal=TERM --kill-after=30s 3600s \
    "$SCRIPT_DIR/build-bundle.sh" | tail -n 1)"
[[ -f "$archive" && -f "$archive.sha256" ]] || die "bundle build did not produce evidence"
readonly archive_name="$(basename -- "$archive")"
[[ "$archive_name" =~ ^aw-provider-poc-([0-9a-f]{40})\.tar\.gz$ ]] ||
    die "bundle archive name does not contain a canonical source commit"
readonly source_commit="${BASH_REMATCH[1]}"
stage="$(timeout --signal=TERM --kill-after=2s 10s \
    "$VM_ROOT/ssh.sh" "mktemp -d '$STAGE_PREFIX.XXXXXXXX'")"
[[ "$stage" =~ ^/home/ubuntu/aw-provider-poc-stage\.[A-Za-z0-9]+$ ]] ||
    die "guest returned an unsafe staging path"
cleanup() {
    if [[ -n "${stage:-}" && "$stage" =~ ^/home/ubuntu/aw-provider-poc-stage\.[A-Za-z0-9]+$ ]]; then
        timeout --signal=TERM --kill-after=2s 10s \
            "$VM_ROOT/ssh.sh" "sudo rm -rf -- '$stage'" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

timeout --signal=TERM --kill-after=5s 120s scp -q -P 2222 \
    -i "$VM_ROOT/id_ed25519" \
    -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=accept-new \
    -o UserKnownHostsFile="$VM_ROOT/known_hosts" \
    "$archive" "$archive.sha256" "$SCRIPT_DIR/vm/guest/provision.sh" \
    ubuntu@127.0.0.1:"$stage/"

timeout --signal=TERM --kill-after=10s 240s "$VM_ROOT/ssh.sh" \
    "chmod 0700 '$stage/provision.sh' && sudo env AW_POC_STAGE='$stage' AW_POC_SOURCE_COMMIT='$source_commit' '$stage/provision.sh'"
action_invocation="$(timeout --signal=TERM --kill-after=2s 15s "$VM_ROOT/ssh.sh" \
    "sudo -iu anolisa herdr --session '$HERDR_SESSION' plugin action invoke '$HERDR_ACTION' --plugin '$HERDR_PLUGIN'")"
printf '%s\n' "$action_invocation"
action_log_id="$(parse_action_log_id <<<"$action_invocation")" ||
    die "could not parse the Herdr action log identity"
action_status=running
action_log_record=
for ((poll_attempt = 1; poll_attempt <= 300; poll_attempt++)); do
    action_log_page="$(timeout --signal=TERM --kill-after=2s 10s "$VM_ROOT/ssh.sh" \
        "sudo -iu anolisa herdr --session '$HERDR_SESSION' plugin log list --plugin '$HERDR_PLUGIN' --limit 20")" ||
        die "could not query Herdr action log $action_log_id"
    action_log_record="$(select_action_log "$action_log_id" <<<"$action_log_page")" ||
        die "Herdr no longer returned action log $action_log_id"
    action_status="$(action_log_status <<<"$action_log_record")" ||
        die "Herdr returned an invalid status for $action_log_id"
    [[ "$action_status" == running ]] || break
    sleep 2
done
[[ "$action_status" != running ]] ||
    die "Herdr action $action_log_id did not settle within 600 seconds"
printf 'Herdr action %s settled as %s\n' "$action_log_id" "$action_status"
render_action_log <<<"$action_log_record"
[[ "$action_status" == succeeded ]] ||
    die "Herdr action $action_log_id failed"

dashboard_output="$(timeout --signal=TERM --kill-after=5s 30s "$VM_ROOT/ssh.sh" \
    "sudo -iu anolisa /opt/anolisa-mvp/aw-provider-poc/current/bin/aw-provider-dashboard")"
printf '%s\n' "$dashboard_output"
for required_heading in \
    '1  CANONICAL PROVIDER TRACE' \
    '2  COSH FINAL ADOPTION' \
    '3  GOVERNED CHECKPOINT'; do
    grep -F "$required_heading" <<<"$dashboard_output" >/dev/null ||
        die "dashboard did not validate all three evidence schemas"
done
verify_shared_services

cleanup
stage=
trap - EXIT
printf 'VM demonstration passed for %s\n' "$source_commit"
