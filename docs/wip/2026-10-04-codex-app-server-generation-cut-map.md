# Codex app-server generation test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut keeps the real WebSocket route, generation gate, and reconciliation proofs unchanged while moving the first generation/retirement scenarios into a private child module.

## Write set

- `crates/collaboration-service/tests/codex_app_server_delivery_route.rs`
- new `crates/collaboration-service/tests/codex_app_server_delivery_route/generation_tests.rs`

## Exact map

Before the cut, `codex_app_server_delivery_route.rs` had 1184 physical lines. Keep `prepared_request`, the held-empty-thread and scheduled-run scenarios, and their shared fixture helper in the parent. Move the complete first three tests from old lines 118–687 into the private child. The parent is 617 lines after the module declaration; the child is 571 lines and uses `use super::*` for shared imports and `prepared_request`.

Moved scenarios:

- `stale_strict_generation_stops_before_evidence_or_native_io`
- `prepared_push_reconciles_only_on_matching_codex_route_identity_and_line`
- `retiring_during_thread_read_returns_retryable_unavailable`

No Codex app-server route/API, generation/evidence/reconciliation behavior, WebSocket fixture, persistence, auth/security, CI, or production behavior changes. The parent remains the Cargo integration-test target; only private module wiring and test ownership changed.

## Proof

- `cargo test -p collaboration-service --test codex_app_server_delivery_route`: exit 0; 9 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0 after removing the formatter-only trailing blank line.
- `cargo check -p collaboration-service --locked`: exit 0.
- `cargo clippy -p collaboration-service --all-targets --locked -- -D warnings`: exit 0.
- Parent prefix/suffix comparison against `git show HEAD:crates/collaboration-service/tests/codex_app_server_delivery_route.rs`: exit 0; unchanged source retained except module wiring.
- Child-body comparison: exit 0 modulo the old terminal blank line removed by formatting; all moved test bodies are preserved.
- Moved test-name inventory: all three names preserved exactly once in the child and once in the pre-cut parent.
- Strict size checker: exit 1 with 12 remaining oversized Rust files; `codex_app_server_delivery_route.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the two Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
