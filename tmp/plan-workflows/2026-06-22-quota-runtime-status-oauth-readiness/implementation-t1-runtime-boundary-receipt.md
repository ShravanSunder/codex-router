# Plan 1A T1 Runtime Boundary Receipt

Timestamp: 2026-06-22T11:09:41-0400

Workspace:

- `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.plan1a-quota-substrate-05bf755`

Scope:

- Plan 1A / T1 only.
- Added CLI seams for `account import-codex-auth`, `account list`, and
  SQLite-only `quota status`.
- Added exact Plan 1A/T1 test names for profile/token activation rows.
- Did not implement Plan 2 OAuth/device-code/keyring login.
- Did not run live OAuth/quota proof.

Files changed:

- `Cargo.lock`
- `Cargo.toml`
- `crates/codex-router-cli/Cargo.toml`
- `crates/codex-router-cli/src/lib.rs`
- `crates/codex-router-cli/src/account.rs`
- `crates/codex-router-cli/src/quota.rs`
- `crates/codex-router-secret-store/src/account_tokens.rs`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/implementation-execute-plan-brief.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/implementation-t1-runtime-boundary-receipt.md`

Red evidence:

- `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_writes_router_owned_state_and_secrets --exact`
  - exit: `100`
  - result: `0 passed, 1 failed`
  - expected failure: `unknown command: account`
- `cargo nextest run -p codex-router-cli -- tests::quota_status_reads_sqlite_rows_without_provider_io --exact`
  - exit: `100`
  - result: `0 passed, 1 failed`
  - expected failure: `unknown command: quota`

Green evidence:

- `cargo nextest run -p codex-router-cli -- tests::account_import_codex_auth_writes_router_owned_state_and_secrets --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::quota_status_reads_sqlite_rows_without_provider_io --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::profile_write_command_requires_approval_flag --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::serve_command_starts_runtime_and_forwards_one_loopback_request --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::profile_print_emits_router_custom_provider_without_home_mutation --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::token_export_and_profile_doctor_redact_router_token_value --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::profile_write_dry_run_previews_named_profile_without_mutation --exact`
  - exit: `0`
  - result: `1 passed`
- `cargo nextest run -p codex-router-cli -- tests::profile_write_approved_writes_only_named_temp_profile_file --exact`
  - exit: `0`
  - result: `1 passed`

Stale-proof evidence:

- Exact-one preflight loop over `1A-00` through `1A-00g`
  - exit: `0`
  - each expected test count: `1`
- `1A-00h` exact-test helper
  - exit: `0`
  - `real_count=1`
  - `missing_count=0`

Validation evidence:

- `cargo fmt --all --check`
  - exit: `0`
- `cargo nextest run -p codex-router-cli`
  - exit: `0`
  - result: `33 passed, 0 skipped`
- `cargo clippy -p codex-router-cli -p codex-router-secret-store --all-targets -- -D warnings`
  - exit: `0`
- `cargo check -p codex-router-cli -p codex-router-secret-store`
  - exit: `0`
- `git diff --check`
  - exit: `0`
- `cargo nextest run --workspace`
  - exit: `0`
  - result: `121 passed, 2 skipped`
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit: `0`

Notes:

- The new quota status renderer uses `comfy-table` for the initial readable
  SQLite-backed table seam. Plan 1B/T10 still owns detailed pace, projected
  runout, weekly-aware bottleneck, expanded window rows, and final status UX.
- The current import writes router-owned account metadata plus router-owned
  access/refresh token secrets. Plan 1A/T3 still owns bundled generation
  atomicity and fail-closed partial-write behavior.
