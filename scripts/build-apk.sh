#!/bin/sh
# build-apk.sh ABI TARGET OUT.apk: build the app's library for TARGET with cargo-ndk, compile
# the picker activity, link the manifest, and put them together as a signed APK at OUT. The
# keystore is $HUSHPHONE_KEYSTORE with its password in $HUSHPHONE_KEYSTORE_PASSWORD; without
# one, the debug keystore in ~/.android is used, made if need be.
#
# The same commit gives the same APK but for its signature: the tools are pinned, entries are
# added in a fixed order, and their times are $SOURCE_DATE_EPOCH.
set -eu
cd "$(dirname "$0")/.."
abi=${1:?ABI}
target=${2:?TARGET}
out=$(realpath -m "${3:?OUT}")
mkdir -p "$(dirname "$out")"
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
tools=$sdk/build-tools/${ANDROID_BUILD_TOOLS_VERSION:-36.0.0}
jar=$sdk/platforms/android-${ANDROID_PLATFORM:-36}/android.jar
: "${SOURCE_DATE_EPOCH:=$(git log -1 --format=%ct)}"
export SOURCE_DATE_EPOCH

version=${VERSION:-$(git describe --tags --always)}
crate=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
code=$(echo "$crate" | awk -F. '{ print $1 * 10000 + $2 * 100 + $3 }')

work=target/apk/$abi
rm -rf "$work"
mkdir -p "$work/classes" "$work/lib/$abi"

# shellcheck disable=SC2086
cargo ndk -t "$abi" -P "${ANDROID_PLATFORM:-36}" build --release --lib --locked ${FEATURES:+--features $FEATURES}
cp "target/$target/release/libhushphone.so" "$work/lib/$abi/"

javac -source 8 -target 8 -Xlint:-options -encoding UTF-8 -bootclasspath "$jar" -d "$work/classes" android/FilePicker.java
"$tools/d8" --release --min-api 28 --lib "$jar" --output "$work" "$work"/classes/network/katzenpost/hushphone/*.class

"$tools/aapt2" link --manifest packaging/android/AndroidManifest.xml -I "$jar" \
    --version-code "$code" --version-name "$version" -o "$work/base.apk"

cd "$work"
touch -d "@$SOURCE_DATE_EPOCH" classes.dex "lib/$abi/libhushphone.so"
cp base.apk unaligned.apk
zip -q -X -D unaligned.apk classes.dex "lib/$abi/libhushphone.so"
"$tools/zipalign" -f 4 unaligned.apk aligned.apk

if [ -n "${HUSHPHONE_KEYSTORE:-}" ]; then
    "$tools/apksigner" sign --ks "$HUSHPHONE_KEYSTORE" --ks-pass env:HUSHPHONE_KEYSTORE_PASSWORD --out "$out" aligned.apk
else
    debug=$HOME/.android/debug.keystore
    if [ ! -f "$debug" ]; then
        mkdir -p "$HOME/.android"
        keytool -genkeypair -keystore "$debug" -storepass android -keypass android -alias androiddebugkey \
            -keyalg RSA -keysize 2048 -validity 10000 -dname CN=Android\ Debug,O=Android,C=US
    fi
    "$tools/apksigner" sign --ks "$debug" --ks-pass pass:android --out "$out" aligned.apk
fi
echo "$out"
