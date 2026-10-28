#!/bin/sh
set -eu

# Build the package, install it, check it, and check that a second build is the same.
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
cd "$root"
packaging/debian/build.sh
apt-get update
apt-get install -y ./dist/hushphone_*.deb
packaging/debian/test.sh
packaging/debian/assert-reproducible.sh
