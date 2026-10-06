# ACP create-settings-order integration-test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut preserves the real ACP subprocess ordering and setting-outcome proofs while moving the trailing outcome cases into a private child module.

## Write set

- `crates/acp-client-runtime/tests/create_settings_order.rs`
- new `crates/acp-client-runtime/tests/create_settings_order/settings_tail_tests.rs`

## Exact map

Before the cut, `create_settings_order.rs` had 1038 physical lines. Keep the shared imports, Python subprocess fixtures, client setup helpers, and the first ordering/gating scenarios through old line 916 in the parent. Move the complete trailing tests from old lines 917–1038 into the private child. The parent is 918 lines after the module declaration; the child is 122 formatted lines and uses `use super::*` for the shared fixtures and imports.

Moved scenarios:

- `explicit_setting_rejection_does_not_gate_session`
- `malformed_legacy_set_mode_response_is_outcome_unknown`
- `disconnected_after_setting_submission_is_outcome_unknown`

No ACP protocol/API, setting state, subprocess fixture, timeout, persistence, auth/security, CI, or production behavior changes. The parent module path remains the existing integration-test target; only test ownership changes.

## Proof

- `cargo test -p acp-client-runtime --test create_settings_order`: exit 0; 12 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p acp-client-runtime --locked`: exit 0.
- `cargo clippy -p acp-client-runtime --all-targets --locked -- -D warnings`: exit 0.
- Parent prefix comparison against `git show HEAD:crates/acp-client-runtime/tests/create_settings_order.rs`: exit 0; first 916 lines byte-equivalent.
- Moved test-name inventory: all three names preserved exactly once in the child and once in the pre-cut parent.
- Strict size checker: exit 1 with 16 remaining oversized Rust files; `create_settings_order.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the two Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
