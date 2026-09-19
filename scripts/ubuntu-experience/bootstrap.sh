#!/usr/bin/env bash
# Install published components before the event; run as the demo user.
set -euo pipefail

if [[ $EUID -eq 0 ]]; then
    echo 'Run as a normal user with sudo access.' >&2
    exit 1
fi
source /etc/os-release
if [[ ${ID:-} != ubuntu || $(uname -m) != x86_64 ]]; then
    echo 'This quick installer requires Ubuntu x86_64 (SkillFS raw artifact).' >&2
    exit 1
fi

sudo timeout 300 apt-get update
sudo timeout 300 apt-get install -y curl ca-certificates git fuse3 python3 python3-pytest
[[ -r /dev/fuse && -w /dev/fuse ]] || {
    echo '/dev/fuse must be accessible to the demo user.' >&2
    exit 1
}

install_tmp=$(mktemp -d)
trap 'rm -rf -- "$install_tmp"' EXIT
export PATH="$HOME/.local/bin:/usr/local/bin:$PATH"
curl --fail --show-error --location --max-time 60 https://get.agentic-os.sh \
    -o "$install_tmp/anolisa.sh"
timeout 300 env ANOLISA_VERSION=0.3.12 bash "$install_tmp/anolisa.sh"
sudo timeout 300 "$HOME/.local/bin/anolisa" --install-mode system install skillfs \
    --backend raw --version 0.4.2
timeout 300 anolisa --install-mode user install tokenless --backend raw --version 0.8.2

if ! command -v qodercli >/dev/null; then
    curl --fail --show-error --location --max-time 60 https://qoder.com/install \
        -o "$install_tmp/qoder.sh"
    timeout 300 bash "$install_tmp/qoder.sh"
fi

python3 "$(dirname "$0")/demo.py" prepare
python3 "$(dirname "$0")/demo.py" check
echo 'Next: python3 scripts/ubuntu-experience/demo.py auth'
echo 'Use /model -> Custom to configure your Token Plan, then exit Qoder.'
