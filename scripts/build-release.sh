#!/bin/sh
# build-release.sh linux|android: build a release into dist/ the way CI does. Paths and dates
# are taken out of the output, so the same commit gives the same bytes wherever it is built.
set -eu
cd "$(dirname "$0")/.."
cargo_home=${CARGO_HOME:-$HOME/.cargo}
here=$(pwd -P)
export RUSTFLAGS="--remap-path-prefix=$cargo_home=/cargo --remap-path-prefix=$here=/hushphone"
export CFLAGS="-ffile-prefix-map=$cargo_home=/cargo -ffile-prefix-map=$here=/hushphone"
export CARGO_INCREMENTAL=0
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}
version=${VERSION:-$(git describe --tags --always)}
rustup toolchain install
mkdir -p dist
case ${1:-} in
linux)
    cargo build --locked --release
    name=hushphone-$version-linux-x86_64
    rm -rf "dist/$name"
    mkdir "dist/$name"
    cp target/release/hushphone README.md LICENSE "dist/$name/"
    tar --sort=name --mtime="@$SOURCE_DATE_EPOCH" --owner=0 --group=0 --numeric-owner \
        --mode=u+rw,go=rX -C dist -cf - "$name" | gzip -9n > "dist/$name.tar.gz"
    rm -rf "dist/$name"
    ;;
android)
    # Slint compiles a Java helper with whichever build tools and platform it finds; pin them.
    export ANDROID_BUILD_TOOLS_VERSION=36.0.0 ANDROID_PLATFORM=36
    for abi in arm64-v8a:aarch64-linux-android x86_64:x86_64-linux-android; do
        scripts/build-apk.sh "${abi%%:*}" "${abi#*:}" "dist/hushphone-$version-${abi%%:*}.apk"
    done
    ;;
*)
    echo "usage: $0 linux|android" >&2
    exit 2
    ;;
esac
