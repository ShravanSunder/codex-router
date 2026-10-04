# Approval dispatch test cut map

Bounded pure-reorganization slice at the current branch. It preserves ACP permission/approval effects, cancellation behavior, fixture scripts and test discovery.

## Write set

- `crates/codex-router-host/src/external_provider_runtime/approval_dispatch_tests.rs`
- new `crates/codex-router-host/src/external_provider_runtime/approval_dispatch_tests/permission_outcome_tests.rs`

## Exact map

Before the cut, `approval_dispatch_tests.rs` had 1146 lines. Keep parent fixtures, broker setup and the first nine tests through old line 1017. Move the final three tests old lines 1018–1146 as complete items:

- `provider_option_sets_keep_order_scope_and_selected_id`
- `output_limit_cancels_pending_permission`
- `provider_exit_cancels_pending_permission_as_provider_retired`

The parent now ends at 889 lines and declares `permission_outcome_tests` as a private path child. The child is 261 formatted lines, starts with `use super::*`, and uses the existing parent fixtures/private helpers. No fixture or protocol script is duplicated.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-host external_provider_runtime::approval_dispatch_tests::permission_outcome_tests::`: exit 0; 3 passed, 0 failed, 141 filtered; integration binaries had no matching filtered tests.
- `cargo check -p codex-router-host --locked` and `cargo clippy -p codex-router-host --all-targets --locked -- -D warnings` remain required after this exact map.
- Parent production/runtime root is unchanged; only test-module declaration and child move are allowed. Existing test attributes, fixture process semantics, permission option identity/order, cancellation reasons and shutdown remain unchanged.

No ACP/API/JSON/schema/auth/security/CI/Cargo/SQLx/migration/production process changes. Strict repository size gate and whole-goal review remain separate obligations.

## Applied result

Parent is 889 lines and child 261 formatted lines. The parent fixture prefix and moved test-name inventory remain unchanged; the child uses only existing private parent helpers. Focused tests passed 3/3; package check and Clippy passed; strict size checker now reports 24 remaining oversized files and no longer lists approval_dispatch_tests.rs.
