#!/bin/sh
set -eu

cd "$(dirname "$0")/../.."
dpkg-buildpackage -b -uc -us "$@"
mkdir -p dist
mv ../hushphone_*.deb dist/
rm -f ../hushphone_*.buildinfo ../hushphone_*.changes
sha256sum dist/*.deb
