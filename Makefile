# SPDX-License-Identifier: AGPL-3.0-only

ANDROID_HOME ?= $(HOME)/Android/Sdk
NDK ?= 30.0.16248370
BUILD_TOOLS = $(ANDROID_HOME)/build-tools/36.0.0
VERSION ?= $(shell git describe --tags --always)
export VERSION

.PHONY: build test check release-linux android-tools release-android apk

build:
	cargo build --release --locked

test:
	SLINT_EMIT_DEBUG_INFO=1 cargo test --locked

check:
	cargo clippy --locked --all-targets -- -D warnings

# The Linux release into dist/, in the image scripts/linux-env.sh prepares.
release-linux:
	scripts/build-release.sh linux

# The SDK pieces and cargo-ndk that the Android release pins.
android-tools:
	yes | $(ANDROID_HOME)/cmdline-tools/latest/bin/sdkmanager \
		"platforms;android-36" "build-tools;36.0.0" "ndk;$(NDK)" >/dev/null
	rustup toolchain install
	cargo install cargo-ndk --locked --version 4.1.2

# The Android release APKs into dist/, one per ABI, each signed and holding its library alone.
release-android:
	ANDROID_NDK_ROOT=$(ANDROID_HOME)/ndk/$(NDK) ANDROID_NDK_HOME=$(ANDROID_HOME)/ndk/$(NDK) scripts/build-release.sh android
	for abi in arm64-v8a x86_64; do \
		apk=dist/hushphone-$(VERSION)-$$abi.apk; \
		$(BUILD_TOOLS)/apksigner verify $$apk && \
		[ "$$(unzip -Z1 $$apk 'lib/*')" = lib/$$abi/libhushphone.so ] || exit 1; \
	done

# A development APK for one ABI (default x86_64, for an emulator), signed with the debug key;
# FEATURES=test-screenshots lets screenshots see its window.
ABI ?= x86_64
TARGET_$(ABI) = $(if $(filter arm64-v8a,$(ABI)),aarch64-linux-android,x86_64-linux-android)
apk:
	ANDROID_NDK_ROOT=$(ANDROID_HOME)/ndk/$(NDK) ANDROID_NDK_HOME=$(ANDROID_HOME)/ndk/$(NDK) \
	ANDROID_BUILD_TOOLS_VERSION=36.0.0 ANDROID_PLATFORM=36 \
		scripts/build-apk.sh $(ABI) $(TARGET_$(ABI)) dist/hushphone-$(ABI).apk
	$(BUILD_TOOLS)/apksigner verify dist/hushphone-$(ABI).apk

include $(dir $(lastword $(MAKEFILE_LIST)))packaging/debian/targets.mk
