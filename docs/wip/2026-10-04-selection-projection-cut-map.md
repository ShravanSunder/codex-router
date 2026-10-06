# Selection projection test cut map

Bounded mechanics-only extraction under the owner continuation instruction. Production selector projection behavior and public module APIs remain unchanged.

## Write set

- `crates/codex-router-state/src/selection_projection.rs`
- new `crates/codex-router-state/src/selection_projection/selection_projection_tests.rs`

## Exact map

Before the cut, `selection_projection.rs` had 1506 lines. Keep the production projection implementation through old line 895. Move the complete inline `#[cfg(test)] mod tests` body from old lines 896–1506 into the private child. The parent declares the child with `#[cfg(test)] #[path = "selection_projection/selection_projection_tests.rs"] mod tests;`.

The parent is 896 lines and the child is 611 formatted lines. All 9 selector projection tests, repositories, SQLite fixtures, policy/credential helpers and assertions remain unchanged. The child remains test-only; no test helper is compiled into the library build.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-state selection_projection::tests::`: exit 0; 9 passed, 0 failed, 155 filtered.
- `cargo check -p codex-router-state --locked`: exit 0, 3.95 seconds.
- `cargo clippy -p codex-router-state --all-targets --locked -- -D warnings`: exit 0.
- Strict size checker: exit 1 with 21 remaining oversized files; `selection_projection.rs` is no longer listed.

A mechanical duplicate-import/test-only-gate correction was applied before final proof. No selector behavior, API, SQLx, migration, concurrency, auth/security, CI or production process changes.
