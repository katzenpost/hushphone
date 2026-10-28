#!/bin/sh
# Prepare the pinned Ubuntu 22.04 image (see LINUX_IMAGE in release.yml) for a Linux release
# build: packages from one Ubuntu snapshot, and rustup. Run as root inside the image.
set -eu
snapshot=20261028T000000Z
cat > /etc/apt/sources.list <<EOF
deb http://snapshot.ubuntu.com/ubuntu/$snapshot jammy main universe
deb http://snapshot.ubuntu.com/ubuntu/$snapshot jammy-updates main universe
deb http://snapshot.ubuntu.com/ubuntu/$snapshot jammy-security main universe
EOF
apt-get -o Acquire::Check-Valid-Until=false update
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates curl git gcc libc6-dev pkg-config libasound2-dev libfontconfig1-dev
curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal
git config --global --add safe.directory '*'
