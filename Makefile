# SPDX-License-Identifier: AGPL-3.0-only

.PHONY: build test check

build:
	cargo build --release --locked

test:
	SLINT_EMIT_DEBUG_INFO=1 cargo test --locked

check:
	cargo clippy --locked --all-targets -- -D warnings

include $(dir $(lastword $(MAKEFILE_LIST)))packaging/debian/targets.mk
