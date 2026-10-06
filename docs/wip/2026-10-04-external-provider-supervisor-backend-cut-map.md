# External provider supervisor backend cut map

Bounded same-crate mechanics-only split under the owner continuation instruction. The cut moves the provider conversation backend trait implementation into a private sibling while retaining supervisor state/admission and settlement helpers in the parent.

## Write set

- `crates/codex-router-host/src/external_provider_supervisor.rs`
- new `crates/codex-router-host/src/external_provider_supervisor/provider_conversation_backend.rs`

## Exact map

Before the cut, `external_provider_supervisor.rs` had 1320 physical lines. Move the complete `ProviderConversationBackend for ExternalProviderSupervisor` implementation (old lines 613–1213; create/load/prompt/cancel/show/wait/reconcile) into `provider_conversation_backend.rs` (603 formatted lines). Keep supervisor state, binding/admission preparation, operation settlement, queue lifecycle, and shared failure helpers in the parent. The parent is 719 formatted lines.

The child is a same-crate private module using `use super::*`; it accesses existing private supervisor state and helper functions without public visibility changes. No provider API, operation admission/effect/reconciliation, persistence, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p codex-router-host --lib`: exit 0; 139 passed, 0 failed, 5 ignored.
- `cargo fmt --all -- --check`: exit 0 after formatter normalization.
- `cargo check -p codex-router-host --locked`: exit 0.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Backend method inventory/source comparison against `git show HEAD:crates/codex-router-host/src/external_provider_supervisor.rs`: all ProviderConversationBackend methods remain in the private child; parent admission/settlement helpers remain intact.
- Strict size checker: exit 1 with 4 remaining oversized Rust files; `external_provider_supervisor.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the Rust parent, child, map, and work trace are staged together. No CI integration or final workspace gate is included in this slice.
