# Provider ACP failure test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut keeps the real provider-process and live-peer route proofs unchanged while moving the final failure scenarios into a private child module.

## Write set

- `crates/codex-router-host/tests/provider_acp_delivery_route.rs`
- new `crates/codex-router-host/tests/provider_acp_delivery_route/provider_failure_tests.rs`

## Exact map

Before the cut, `provider_acp_delivery_route.rs` had 1121 physical lines. Keep the shared provider binding, route, fixture scripts, operation-store setup and the first six delivery tests through old line 964 in the parent. Move the complete final tests from old lines 966–1121 (including the separating blank line) into the private child. The parent is 967 lines after the module declaration; the child is 158 lines and uses `use super::*` for shared route fixtures and helpers.

Moved scenarios:

- `provider_process_transport_failure_remains_retryable`
- `live_peer_recheck_prevents_provider_load`

No provider ACP route/API, delivery effect, operation-store, subprocess fixture, persistence, auth/security, CI, or production behavior changes. The parent remains the Cargo integration-test target; only private module wiring and test ownership changed.

## Proof

- `cargo test -p codex-router-host --test provider_acp_delivery_route`: exit 0; 8 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-host --locked`: exit 0.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Prefix comparison against `git show HEAD:crates/codex-router-host/tests/provider_acp_delivery_route.rs`: exit 0; first 965 lines byte-equivalent.
- Child-tail comparison: exit 0; child body equals old lines 966–1121 after the one-line `use super::*` import.
- Moved test-name inventory: both names preserved exactly once in the child and once in the pre-cut parent.
- Strict size checker: exit 1 with 13 remaining oversized Rust files; `provider_acp_delivery_route.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the two Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
