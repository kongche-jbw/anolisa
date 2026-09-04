#!/usr/bin/env bash

set -Eeuo pipefail
IFS=$'\n\t'
umask 077

readonly STAGE="${AW_POC_STAGE:-}"
readonly SOURCE_COMMIT="${AW_POC_SOURCE_COMMIT:-}"
readonly INSTALL_ROOT=/opt/anolisa-mvp/aw-provider-poc
readonly INTERACTIVE_USER=anolisa
readonly INTERACTIVE_HOME=/home/anolisa
readonly USER_EVIDENCE_ROOT="$INTERACTIVE_HOME/.local/state/aw-provider-poc"
readonly HERDR=/usr/local/bin/herdr
readonly SERVICE=cosh-gateway@aw-provider-poc.service
readonly SERVICE_FILE="/etc/systemd/system/$SERVICE"
readonly WORKSPACE=/var/lib/anolisa-agent-work/workspaces/interactive-agent
readonly WS_CKPT_SOCKET=/run/ws-ckpt-agent-work/ws-ckpt.sock
readonly EXACT_ADOPTION_TEST=\
'core::tests::real_providers_commit_effective_history_and_adoption_evidence'

die() {
    printf 'provision-aw-provider-poc: ERROR: %s\n' "$*" >&2
    exit 1
}

unlink_plugin() {
    timeout --signal=TERM --kill-after=2s 15s \
        runuser -u "$INTERACTIVE_USER" -- env \
        HOME="$INTERACTIVE_HOME" \
        PATH="$INTERACTIVE_HOME/.local/bin:/usr/local/bin:/opt/anolisa-mvp/bin:/usr/bin:/bin" \
        "$HERDR" plugin unlink anolisa.aw-provider-poc >/dev/null 2>&1 || true
}

gateway_start_failed() {
    timeout --signal=TERM --kill-after=2s 15s \
        systemctl status "$SERVICE" --no-pager -l >&2 || true
    timeout --signal=TERM --kill-after=2s 15s \
        journalctl -u "$SERVICE" -n 80 --no-pager >&2 || true
    timeout --signal=TERM --kill-after=2s 30s \
        systemctl disable --now "$SERVICE" >/dev/null 2>&1 || true
    unlink_plugin
    die "dedicated checkpoint Gateway did not become ready"
}

post_start_failed() {
    timeout --signal=TERM --kill-after=2s 30s \
        systemctl disable --now "$SERVICE" >/dev/null 2>&1 || true
    unlink_plugin
    die "$1"
}

[[ "$(id -u)" -eq 0 ]] || die "run through sudo"
[[ "$(uname -s)" == Linux && "$(uname -m)" == aarch64 ]] ||
    die "this bundle requires Linux aarch64"
[[ "$STAGE" =~ ^/home/ubuntu/aw-provider-poc-stage\.[A-Za-z0-9]+$ ]] ||
    die "unsafe staging path"
[[ -d "$STAGE" && ! -L "$STAGE" ]] || die "staging directory is unsafe"
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || die "invalid source commit"
id "$INTERACTIVE_USER" >/dev/null 2>&1 || die "interactive user is unavailable"
[[ -x "$HERDR" ]] || die "Herdr is unavailable: $HERDR"
[[ -S "$WS_CKPT_SOCKET" ]] || die "ws-ckpt socket is unavailable: $WS_CKPT_SOCKET"
[[ -e "$WORKSPACE" ]] || die "workspace registration path is unavailable: $WORKSPACE"
if [[ -e "$INSTALL_ROOT" || -L "$INSTALL_ROOT" ]]; then
    [[ -d "$INSTALL_ROOT" && ! -L "$INSTALL_ROOT" ]] ||
        die "existing install root is unsafe: $INSTALL_ROOT"
fi
if [[ -e "$SERVICE_FILE" || -L "$SERVICE_FILE" ]]; then
    [[ -f "$SERVICE_FILE" && ! -L "$SERVICE_FILE" ]] ||
        die "existing service file is unsafe: $SERVICE_FILE"
fi
if [[ -e "$USER_EVIDENCE_ROOT" || -L "$USER_EVIDENCE_ROOT" ]]; then
    [[ -d "$USER_EVIDENCE_ROOT" && ! -L "$USER_EVIDENCE_ROOT" ]] ||
        die "existing user evidence root is unsafe: $USER_EVIDENCE_ROOT"
fi
readonly archive="$STAGE/aw-provider-poc-$SOURCE_COMMIT.tar.gz"
readonly checksum="$archive.sha256"
[[ -f "$archive" && ! -L "$archive" ]] || die "bundle archive is missing"
[[ -f "$checksum" && ! -L "$checksum" ]] || die "bundle checksum is missing"
(cd "$STAGE" && sha256sum --check "$(basename "$checksum")") ||
    die "bundle checksum does not match"

python3 - "$archive" <<'PY'
import pathlib
import sys
import tarfile

archive = pathlib.Path(sys.argv[1])
with tarfile.open(archive) as bundle:
    for member in bundle.getmembers():
        path = pathlib.PurePosixPath(member.name)
        if path.is_absolute() or ".." in path.parts:
            raise SystemExit(f"unsafe archive member: {member.name}")
        if member.issym() or member.islnk() or member.isdev():
            raise SystemExit(f"unsupported archive member type: {member.name}")
PY

install -d -o root -g root -m 0555 "$INSTALL_ROOT" "$INSTALL_ROOT/releases"
release="$INSTALL_ROOT/releases/$SOURCE_COMMIT"
if [[ -e "$release" || -L "$release" ]]; then
    [[ -d "$release" && ! -L "$release" ]] || die "existing release path is unsafe"
else
    candidate="$(mktemp -d "$INSTALL_ROOT/releases/.install.XXXXXXXX")"
    trap '[[ -n "${candidate:-}" && -d "$candidate" ]] && rm -rf -- "$candidate"' EXIT
    tar -C "$candidate" -xzf "$archive"
    python3 - "$candidate/build-info.json" "$SOURCE_COMMIT" <<'PY'
import json
import pathlib
import sys

evidence = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
if evidence.get("schema") != "aw.provider.vm-bundle/v1":
    raise SystemExit("unsupported bundle evidence schema")
if evidence.get("source_commit") != sys.argv[2]:
    raise SystemExit("bundle source commit does not match deployment")
if evidence.get("architecture") != "aarch64":
    raise SystemExit("bundle architecture is not aarch64")
PY
    chown -R root:root "$candidate"
    find "$candidate" -type d -exec chmod 0555 {} +
    find "$candidate" -type f -exec chmod a-w,u-s,g-s {} +
    mv "$candidate" "$release"
    candidate=
    trap - EXIT
fi

for path in \
    "$release/bin/aw-provider-host" \
    "$release/bin/aw-cosh-hook" \
    "$release/bin/aw-ledger" \
    "$release/bin/aw-checkpoint-demo" \
    "$release/bin/aw-cosh-adoption-demo" \
    "$release/bin/aw-provider-dashboard" \
    "$release/bin/aw-provider-demo" \
    "$release/bin/aw-provider-e2e" \
    "$release/bin/agent-sec-cli" \
    "$release/bin/cosh-core" \
    "$release/bin/cosh-gateway" \
    "$release/bin/cosh-final-adoption-test" \
    "$release/bin/tokenless" \
    "$release/build-info.json" \
    "$release/config/cosh-system.toml" \
    "$release/config/cosh-gateway@aw-provider-poc.service" \
    "$release/providers/agent-sec-core/provider.toml" \
    "$release/providers/tokenless/provider.toml"; do
    [[ -f "$path" && ! -L "$path" ]] || die "installed release is incomplete: $path"
done
for executable in \
    "$release/bin/aw-provider-host" \
    "$release/bin/aw-cosh-hook" \
    "$release/bin/aw-ledger" \
    "$release/bin/aw-checkpoint-demo" \
    "$release/bin/aw-cosh-adoption-demo" \
    "$release/bin/aw-provider-dashboard" \
    "$release/bin/aw-provider-demo" \
    "$release/bin/aw-provider-e2e" \
    "$release/bin/agent-sec-cli" \
    "$release/bin/cosh-core" \
    "$release/bin/cosh-gateway" \
    "$release/bin/cosh-final-adoption-test" \
    "$release/bin/tokenless" \
    "$release/e2e-repository/src/agent-sec-core/agent-sec-cli/.venv/bin/agent-sec-cli" \
    "$release/e2e-repository/src/tokenless/target/debug/tokenless"; do
    [[ -x "$executable" ]] || die "installed executable is not runnable: $executable"
done

python3 - \
    "$release/build-info.json" \
    "$release/bin/cosh-final-adoption-test" \
    "$SOURCE_COMMIT" \
    "$EXACT_ADOPTION_TEST" <<'PY'
import hashlib
import json
import pathlib
import sys

evidence = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
test = pathlib.Path(sys.argv[2])
if evidence.get("schema") != "aw.provider.vm-bundle/v1":
    raise SystemExit("installed release has an unsupported evidence schema")
if evidence.get("source_commit") != sys.argv[3]:
    raise SystemExit("installed release is not bound to the requested source commit")
if evidence.get("architecture") != "aarch64":
    raise SystemExit("installed release is not built for aarch64")
if evidence.get("providers") != ["agent-sec-core", "tokenless"]:
    raise SystemExit("installed release does not identify the two real Providers")
if evidence.get("state_provider") != "gateway/workspace-checkpoint-v1":
    raise SystemExit("installed release does not identify the checkpoint State Provider")
expected = evidence.get("cosh_final_adoption_test", {})
if expected.get("exact_test") != sys.argv[4]:
    raise SystemExit("installed release does not identify the exact adoption test")
if expected.get("sha256") != hashlib.sha256(test.read_bytes()).hexdigest():
    raise SystemExit("cosh-core final-adoption test digest does not match build evidence")
if expected.get("size") != test.stat().st_size:
    raise SystemExit("cosh-core final-adoption test size does not match build evidence")
PY

ln -sfn "releases/$SOURCE_COMMIT" "$INSTALL_ROOT/current.next"
mv -Tf "$INSTALL_ROOT/current.next" "$INSTALL_ROOT/current"

install -o root -g root -m 0444 \
    "$release/config/cosh-gateway@aw-provider-poc.service" \
    "$SERVICE_FILE"
timeout --signal=TERM --kill-after=2s 15s systemctl daemon-reload
timeout --signal=TERM --kill-after=2s 15s systemctl enable "$SERVICE" >/dev/null
timeout --signal=TERM --kill-after=2s 30s systemctl restart "$SERVICE" ||
    gateway_start_failed
for ((readiness_attempt = 1; readiness_attempt <= 60; readiness_attempt++)); do
    if timeout --signal=TERM --kill-after=1s 2s \
        systemctl is-active --quiet "$SERVICE" && \
        [[ -S /run/anolisa-aw-provider-poc/gateway.sock ]]; then
        break
    fi
    sleep 0.5
done
if ! timeout --signal=TERM --kill-after=1s 2s \
    systemctl is-active --quiet "$SERVICE" || \
    [[ ! -S /run/anolisa-aw-provider-poc/gateway.sock ]]; then
    gateway_start_failed
fi
effective_properties="$(timeout --signal=TERM --kill-after=2s 15s \
    systemctl show "$SERVICE" --no-pager \
    --property=RestrictSUIDSGID,TemporaryFileSystem,InaccessiblePaths)" ||
    post_start_failed "could not inspect dedicated Gateway containment"
readonly effective_properties
for expected_property in \
    'RestrictSUIDSGID=no' \
    'TemporaryFileSystem=/dev/shm:ro,nosuid,nodev,noexec' \
    'InaccessiblePaths=/run/user'; do
    grep -Fqx -- "$expected_property" <<<"$effective_properties" ||
        post_start_failed \
            "dedicated Gateway containment property mismatch: $expected_property"
done

install -d -o "$INTERACTIVE_USER" -g "$INTERACTIVE_USER" -m 0700 \
    "$USER_EVIDENCE_ROOT" ||
    post_start_failed "could not prepare the user evidence root"
install -d -o "$INTERACTIVE_USER" -g "$INTERACTIVE_USER" -m 0700 \
    "$USER_EVIDENCE_ROOT/runs" \
    "$USER_EVIDENCE_ROOT/adoption" \
    "$USER_EVIDENCE_ROOT/checkpoints" ||
    post_start_failed "could not prepare user evidence directories"
[[ "$(stat -c '%U:%G:%a' "$USER_EVIDENCE_ROOT")" == \
    "$INTERACTIVE_USER:$INTERACTIVE_USER:700" ]] ||
    post_start_failed "user evidence root has the wrong ownership or mode"
unlink_plugin
timeout --signal=TERM --kill-after=2s 30s runuser -u "$INTERACTIVE_USER" -- env \
    HOME="$INTERACTIVE_HOME" \
    PATH="$INTERACTIVE_HOME/.local/bin:/usr/local/bin:/opt/anolisa-mvp/bin:/usr/bin:/bin" \
    "$HERDR" plugin link "$INSTALL_ROOT/current/herdr-plugin" --enabled ||
    post_start_failed "Herdr did not link the AW Provider PoC plugin"
timeout --signal=TERM --kill-after=2s 15s runuser -u "$INTERACTIVE_USER" -- env \
    HOME="$INTERACTIVE_HOME" \
    PATH="$INTERACTIVE_HOME/.local/bin:/usr/local/bin:/opt/anolisa-mvp/bin:/usr/bin:/bin" \
    "$HERDR" plugin list | grep -F 'anolisa.aw-provider-poc' >/dev/null ||
    post_start_failed "Herdr did not admit the AW Provider PoC plugin"

timeout --signal=TERM --kill-after=2s 15s runuser -u "$INTERACTIVE_USER" -- \
    "$release/bin/cosh-gateway" task \
    --socket /run/anolisa-aw-provider-poc/gateway.sock \
    --output jsonl admission | \
    grep -F '"profile_id":"workspace-checkpoint-v1"' >/dev/null ||
    post_start_failed "dedicated Gateway did not admit the checkpoint profile"

printf 'Installed AW Provider PoC release %s\n' "$SOURCE_COMMIT"
