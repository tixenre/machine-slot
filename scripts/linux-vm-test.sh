#!/bin/bash
# machine-slot's tests on Linux, in a lima VM of its own (Ubuntu LTS, only
# this checkout mounted, read-only). Run it through the queue:
#   machine-slot shared "machine-slot: linux tests" --timeout 25m -- scripts/linux-vm-test.sh
# `limactl delete machine-slot-ci` removes the VM.
set -euo pipefail
VM=machine-slot-ci
SRC="$(cd "$(dirname "$0")/.." && pwd)"
if ! limactl list -q | grep -qx "$VM"; then
    limactl create --name="$VM" --tty=false --cpus=4 --memory=4 --disk=20 \
        --set ".mounts=[{\"location\":\"$SRC\"}] | .containerd.system=false | .containerd.user=false" \
        template:ubuntu-lts
fi
limactl list "$VM" --format '{{.Status}}' | grep -q Running || limactl start --tty=false "$VM"
# Stopped again afterwards: a running VM holds its memory.
trap 'limactl stop "$VM" >/dev/null 2>&1' EXIT
limactl shell "$VM" bash -lc '
set -e
command -v cargo >/dev/null || { curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c clippy >/dev/null; }
. "$HOME/.cargo/env"
command -v cc >/dev/null && command -v python3 >/dev/null || { sudo apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq gcc python3 >/dev/null; }
rm -rf ~/ms && cp -R '"$SRC"' ~/ms && cd ~/ms && rm -rf target
uname -srm; cargo --version
cargo clippy --release --all-targets -- -D warnings
MACHINE_SLOT_SETTLE=0 cargo test --release --no-fail-fast 2>&1
'
