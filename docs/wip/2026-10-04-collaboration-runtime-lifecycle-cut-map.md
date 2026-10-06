# Collaboration runtime lifecycle cut map

Bounded same-crate mechanics-only split under the owner continuation instruction. The cut moves lifecycle teardown methods into a private sibling module while preserving the `CollaborationRuntime` state, private fields, and public behavior.

## Write set

- `crates/codex-router-host/src/collaboration_runtime.rs`
- new `crates/codex-router-host/src/collaboration_runtime/lifecycle.rs`

## Exact map

Before the cut, `collaboration_runtime.rs` had 1100 physical lines. Keep inputs, provider binding constructors, runtime startup/composition, backend readiness/publication and helper functions in the parent. Move the complete `listener_failure` and `shutdown` inherent methods (old lines 917–1058) into `collaboration_runtime/lifecycle.rs` as a sibling `impl CollaborationRuntime`. The parent is 962 formatted lines; the child is 146 formatted lines.

The child remains inside the same crate module tree and accesses the existing private runtime fields through the parent-child privacy boundary. The `listener_failure` documentation moved with its method. No public API, lifecycle ordering, storage, network, auth/security, concurrency, CI, or production behavior changed.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-host collaboration_runtime --lib`: exit 0; compilation passed and no tests matched this module filter (0 passed, 0 failed, 144 filtered).
- `cargo check -p codex-router-host --locked`: exit 0.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Lifecycle method inventory/source comparison against `git show HEAD:crates/codex-router-host/src/collaboration_runtime.rs`: exit 0; both moved methods and their bodies remain present in the child, with the documentation comment moved alongside `listener_failure`.
- Strict size checker: exit 1 with 8 remaining oversized Rust files; `collaboration_runtime.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the Rust parent, child, map, and work trace are staged together. No CI integration or final workspace gate is included in this slice.
