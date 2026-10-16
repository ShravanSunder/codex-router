#!/bin/sh
# Run from the repository root. Tools remain checkout-local; nothing is installed.
set -eu
test -f crates/codex-router-quota-reset-test-harness/Cargo.toml
tool_root="${QUOTA_STORYBOARD_TOOL_ROOT:-$PWD/tmp/quota-real-tui-tools-2026-10-12}"
export QUOTA_STORYBOARD_VHS="$tool_root/vhs/vhs/0.12.1/bin/vhs"
export QUOTA_STORYBOARD_TTYD="$tool_root/ttyd/ttyd/1.7.7_14/bin/ttyd"
export QUOTA_STORYBOARD_IMAGE_VERIFIER="$tool_root/verify-screen-image"
export QUOTA_STORYBOARD_DYLD_LIBRARY_PATH="$tool_root/libwebsockets/libwebsockets/5.0.0_2/lib:/opt/homebrew/opt/json-c/lib:/opt/homebrew/opt/libuv/lib:/opt/homebrew/opt/openssl@4/lib"
export QUOTA_STORYBOARD_OUTPUT="${QUOTA_STORYBOARD_OUTPUT:-$PWD/tmp/quota-live-storyboard-$(date -u +%Y-%m-%dT%H%M%SZ)-$$}"
test -x "$QUOTA_STORYBOARD_VHS"
test -x "$QUOTA_STORYBOARD_TTYD"
xcrun swiftc -parse-as-library -module-cache-path "$tool_root/swift-module-cache" \
    crates/codex-router-quota-reset-test-harness/tests/quota_storyboard_capture_test/verify_screen_image.swift \
    -o "$QUOTA_STORYBOARD_IMAGE_VERIFIER"
env -u CC -u CXX -u LDFLAGS -u CPPFLAGS \
    PATH="/opt/homebrew/opt/rustup/bin:$PATH" \
    CARGO_TARGET_DIR="$PWD/.build-agent-1" \
    cargo test -p codex-router-quota-reset-test-harness \
    --test quota_storyboard_capture_test --locked --offline \
    -- --include-ignored --nocapture --test-threads=1
