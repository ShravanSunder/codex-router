# External provider supervisor integration-test cut map

Bounded mechanics-only integration-test split under the owner continuation instruction. The cut keeps the real external-provider subprocess, operation-store, settings, shutdown, approval, authentication, and reconciliation proofs unchanged while separating scenario families into private children.

## Write set

- `crates/codex-router-host/tests/external_provider_supervisor.rs`
- new `crates/codex-router-host/tests/external_provider_supervisor/settings_projection_tests.rs`
- new `crates/codex-router-host/tests/external_provider_supervisor/lifecycle_tests.rs`

## Exact map

Before the cut, `external_provider_supervisor.rs` had 1729 physical lines. Keep imports, test macros, endpoint/binding helpers, fixture scripts, and operation helpers in the parent. Move the first settings projection test (old lines 171–403) into `settings_projection_tests.rs`, and move the complete lifecycle/operation test block (old lines 797–1729) into `lifecycle_tests.rs`. The parent is 568 lines; the children are 234 and 935 lines and use `use super::*` for shared helpers and fixtures.

Moved scenario families:

- settings projection and invalid/partial settings outcomes (`provider_create_projects_effective_partial_and_invalid_settings`)
- admission/cancel, shutdown draining, permission callbacks, retired bindings, load/resume/close, response loss, authentication, and provider rejection lifecycle tests (11 existing tests)

No external-provider API, operation effect/reconciliation, subprocess fixture, persistence, auth semantics, CI, or production behavior changes. The parent remains the Cargo integration-test target; only private module wiring and test ownership changed.

## Proof

- `cargo test -p codex-router-host --test external_provider_supervisor`: exit 0; 12 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0 after removing the formatter-only trailing blank line from the settings child.
- `cargo check -p codex-router-host --locked`: exit 0.
- `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings`: exit 0.
- Parent retained-source comparison against `git show HEAD:crates/codex-router-host/tests/external_provider_supervisor.rs`: exit 0; helpers and fixtures are unchanged except module declarations.
- Child-body comparisons: lifecycle body is exact; settings body is exact modulo the old terminal blank line removed by formatting.
- Strict size checker: exit 1 with 11 remaining oversized Rust files; `external_provider_supervisor.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the three Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
