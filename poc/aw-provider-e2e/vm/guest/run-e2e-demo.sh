#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly POC_ROOT="$(readlink -f /opt/anolisa-mvp/aw-provider-poc/current)"
case "$POC_ROOT" in
    /opt/anolisa-mvp/aw-provider-poc/releases/*) ;;
    *) printf 'aw-provider-e2e: unsafe release path: %s\n' "$POC_ROOT" >&2; exit 1 ;;
esac

printf '\n[1/3] Canonical Provider field trace\n'
timeout --signal=TERM --kill-after=5s 90s "$POC_ROOT/bin/aw-provider-demo"

printf '\n[2/3] Real COSH final-adoption trace\n'
timeout --signal=TERM --kill-after=5s 180s "$POC_ROOT/bin/aw-cosh-adoption-demo"

printf '\n[3/3] Governed Gateway checkpoint trace\n'
timeout --signal=TERM --kill-after=5s 120s "$POC_ROOT/bin/aw-checkpoint-demo"

printf '\nAll three traces passed. Open the Herdr evidence pane for the compact view.\n'
