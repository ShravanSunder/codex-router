#!/bin/sh
# One serial, opt-in capture of both actual compiled TUIs. No global installs.
set -eu
test -f crates/codex-router-quota-reset-test-harness/Cargo.toml
: "${QUOTA_STORYBOARD_TOOL_ROOT:?Nominate the verified private tools root}"
: "${QUOTA_STORYBOARD_OUTPUT:?Nominate a fresh owned output directory}"
: "${CARGO_TARGET_DIR:?Nominate the allocated private capture target}"
: "${TMPDIR:?Nominate the allocated private task temp directory}"
export QUOTA_STORYBOARD_VHS="$QUOTA_STORYBOARD_TOOL_ROOT/vhs/vhs/0.12.1/bin/vhs"
export QUOTA_STORYBOARD_TTYD="$QUOTA_STORYBOARD_TOOL_ROOT/ttyd/ttyd/1.7.7_14/bin/ttyd"
export QUOTA_STORYBOARD_IMAGE_VERIFIER="$QUOTA_STORYBOARD_OUTPUT/verify-provider-filter-screen"
export QUOTA_STORYBOARD_DYLD_LIBRARY_PATH="$QUOTA_STORYBOARD_TOOL_ROOT/libwebsockets/libwebsockets/5.0.0_2/lib:/opt/homebrew/opt/json-c/lib:/opt/homebrew/opt/libuv/lib:/opt/homebrew/opt/openssl@4/lib"
test -x "$QUOTA_STORYBOARD_VHS"
test -x "$QUOTA_STORYBOARD_TTYD"
mkdir -p "$QUOTA_STORYBOARD_OUTPUT"
chmod 700 "$QUOTA_STORYBOARD_OUTPUT"
xcrun swiftc -parse-as-library -module-cache-path "$QUOTA_STORYBOARD_OUTPUT/swift-module-cache" \
    crates/codex-router-quota-reset-test-harness/tests/quota_storyboard_capture_test/verify_screen_image.swift \
    -o "$QUOTA_STORYBOARD_IMAGE_VERIFIER"
env -u CC -u CXX -u LDFLAGS -u CPPFLAGS \
    PATH="/opt/homebrew/opt/rustup/bin:$PATH" CARGO_BUILD_JOBS=1 \
    cargo test -p codex-router-quota-reset-test-harness \
    --test quota_storyboard_capture_test compiled_compact_current_storyboard_exports_both_real_tuis \
    --locked --offline -j 1 -- --include-ignored --nocapture --test-threads=1
