#!/bin/sh
set -eu

# Build again from a copy in another directory: the packages must be the same.
root=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
again=${REBUILD_DIR:-/tmp/hushphone-rebuild}
a=$(sha256sum "$root"/dist/*.deb | awk '{print $1}' | sort | sha256sum | cut -d' ' -f1)

rm -rf "$again"
mkdir -p "$again"
tar -C "$root" --exclude=./.git --exclude=./target --exclude=./dist --exclude=./shots \
    --exclude=./debian/cargo-home --exclude=./debian/hushphone --exclude=./debian/.debhelper \
    -cf - . | tar -C "$again" -xf -
cd "$again"
packaging/debian/build.sh
b=$(sha256sum "$again"/dist/*.deb | awk '{print $1}' | sort | sha256sum | cut -d' ' -f1)

printf 'first  %s\nsecond %s\n' "$a" "$b"
test "$a" = "$b"
