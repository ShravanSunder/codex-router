# Account command decomposition cut map

Bounded mechanics-only split under the owner continuation instruction. The cut separates account option parsing and account test suites from the command implementation while preserving all CLI contracts and credential/state behavior.

## Write set

- `crates/codex-router-cli/src/account.rs`
- new `crates/codex-router-cli/src/account_options.rs`
- new `crates/codex-router-cli/src/account_tests.rs`

## Exact map

Before the cut, `account.rs` had 1370 physical lines. Move the three existing `#[cfg(test)]` suites (`account_status_error_tests`, `account_provider_cli_tests`, and `claude_login_glue_tests`) into private `account_tests.rs` (207 lines). Move `AccountLoginOptions`, `AccountRootOptions`, `AccountSetWeeklyFloorOptions`, and `AccountStatusOptions` plus their parser methods into same-crate `account_options.rs` (213 formatted lines), exposing only `pub(super)` types/methods needed by `AccountCommand::parse`; the provider selector field is `pub(super)` because the parent directly applies its default.

The parent is 962 formatted lines. Explicit `#[path]` attributes preserve sibling file locations and avoid Rust's default `account/account_options.rs` lookup. No account CLI/API, OAuth, credential activation, state storage, auth/security, CI, or production behavior changes.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-cli account::account_tests:: --lib`: exit 0; 6 passed, 0 failed.
- `cargo check -p codex-router-cli --locked`: exit 0.
- `cargo clippy -p codex-router-cli --all-targets --features keychain-test-support --locked -- -D warnings`: exit 0.
- Full `cargo test -p codex-router-cli account --lib`: 84 passed, 2 unrelated sandbox tests failed because the host rejects `sandbox-exec` with `Operation not permitted`; no moved account test failed. This environment blocker is recorded and not treated as source proof.
- Moved test-name and option-type inventory: all three suites and four option types/method groups remain present in the new children.
- Strict size checker: exit 1 with 7 remaining oversized Rust files; `account.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the three Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
