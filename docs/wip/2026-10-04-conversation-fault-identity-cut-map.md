# Conversation fault identity test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut keeps the real compiled CLI and ACP/control fixture proofs unchanged while moving the trailing identity-source scenarios into a private child module.

## Write set

- `crates/agent-collaboration/tests/conversation_fault_entry_paths.rs`
- new `crates/agent-collaboration/tests/conversation_fault_entry_paths/identity_override_tests.rs`

## Exact map

Before the cut, `conversation_fault_entry_paths.rs` had 1274 physical lines. Keep the shared JSON result parsers, subprocess fixtures, response-loss tests, and resumed-prompt helper/tests through old line 848 in the parent. Move the complete tests from old lines 849–1274 into the private child. The parent is 850 lines after the module declaration; the child is 428 lines and uses `use super::*` for shared imports and parsers.

Moved scenarios:

- `conversation_create_without_identity_or_from_reports_unavailable`
- `conversation_create_from_supplies_created_by_without_env`

No CLI protocol/API, SessionRef identity, ACP dispatch, control manifest, subprocess fixture, persistence, auth/security, CI, or production behavior changes. This is a conventional integration-test target; the parent remains the Cargo test target and the child is private module wiring only.

## Proof

- `cargo test -p agent-collaboration --test conversation_fault_entry_paths`: exit 0; 9 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p agent-collaboration --locked`: exit 0.
- `cargo clippy -p agent-collaboration --all-targets --locked -- -D warnings`: exit 0.
- Prefix comparison against `git show HEAD:crates/agent-collaboration/tests/conversation_fault_entry_paths.rs`: exit 0; first 848 lines byte-equivalent.
- Child-tail comparison: exit 0; moved body equals old lines 849–1274 after the one-line `use super::*` import.
- Moved test-name inventory: both names preserved exactly once in the child and once in the pre-cut parent.
- Strict size checker: exit 1 with 15 remaining oversized Rust files; `conversation_fault_entry_paths.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the two Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
