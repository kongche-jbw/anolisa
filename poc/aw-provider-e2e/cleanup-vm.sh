#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly VM_ROOT="${AW_POC_VM_ROOT:-/home/kongche/anolisa/ws/ubuntu-26.04-vm}"
readonly STAGE_PREFIX=/home/ubuntu/aw-provider-poc-cleanup

die() {
    printf 'cleanup-aw-provider-poc-vm: ERROR: %s\n' "$*" >&2
    exit 1
}

guest_args=()
confirmed=false
while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --yes) confirmed=true; guest_args+=(--yes) ;;
        --purge-evidence) guest_args+=(--purge-evidence) ;;
        --purge-checkpoints) guest_args+=(--purge-checkpoints) ;;
        --help|-h)
            bash "$SCRIPT_DIR/vm/guest/cleanup.sh" --help
            exit 0
            ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done
[[ "$confirmed" == true ]] || die "review the scope, then pass --yes"
[[ -x "$VM_ROOT/ssh.sh" ]] || die "VM SSH helper is missing under $VM_ROOT"
[[ -f "$SCRIPT_DIR/vm/guest/cleanup.sh" ]] || die "guest cleanup script is missing"
timeout --signal=TERM --kill-after=2s 10s "$VM_ROOT/ssh.sh" true >/dev/null 2>&1 ||
    die "the existing VM is not reachable; this script never starts or stops QEMU"

stage="$(timeout --signal=TERM --kill-after=2s 10s \
    "$VM_ROOT/ssh.sh" "mktemp -d '$STAGE_PREFIX.XXXXXXXX'")"
[[ "$stage" =~ ^/home/ubuntu/aw-provider-poc-cleanup\.[A-Za-z0-9]+$ ]] ||
    die "guest returned an unsafe staging path"
cleanup() {
    if [[ -n "${stage:-}" && "$stage" =~ ^/home/ubuntu/aw-provider-poc-cleanup\.[A-Za-z0-9]+$ ]]; then
        timeout --signal=TERM --kill-after=2s 10s \
            "$VM_ROOT/ssh.sh" "sudo rm -rf -- '$stage'" >/dev/null 2>&1 || true
    fi
}
trap cleanup EXIT

timeout --signal=TERM --kill-after=2s 30s scp -q -P 2222 \
    -i "$VM_ROOT/id_ed25519" \
    -o IdentitiesOnly=yes \
    -o StrictHostKeyChecking=accept-new \
    -o UserKnownHostsFile="$VM_ROOT/known_hosts" \
    "$SCRIPT_DIR/vm/guest/cleanup.sh" ubuntu@127.0.0.1:"$stage/cleanup.sh"

remote_arguments=
for argument in "${guest_args[@]}"; do
    printf -v quoted '%q' "$argument"
    remote_arguments+=" $quoted"
done
timeout --signal=TERM --kill-after=5s 180s "$VM_ROOT/ssh.sh" \
    "chmod 0700 '$stage/cleanup.sh' && sudo '$stage/cleanup.sh'$remote_arguments"

cleanup
stage=
trap - EXIT
