# Quota refresh helper cut map

Bounded mechanics-only slice under the owner continuation instruction. This separates helper/freshness responsibilities while preserving the quota refresh orchestrator, errors, persistence and public reexports.

## Write set

- `crates/codex-router-cli/src/quota/quota_refresh_service.rs`
- new `crates/codex-router-cli/src/quota/quota_refresh_service/quota_refresh_helpers.rs`

## Exact map

Before the cut, `quota_refresh_service.rs` had 1091 lines. Keep the report/context/schedule types and primary refresh orchestrator through old line 911. Move old lines 912–1091 into the private helper child:

- `SupersededResponsesFloorRead`
- `notify_weekly_floor_from_latest_committed_responses`
- `weekly_quota_floor_intent`
- `begin_credit_refresh_attempt_for_current_generation`
- `record_superseded_account_refresh`
- `quota_refresh_diagnostic_account_label`
- `quota_refresh_error_class`
- `freshness_tests` and its single test

The parent adds `mod quota_refresh_helpers; use quota_refresh_helpers::*;` after the moved region. Helper functions and the `SupersededResponsesFloorRead` fields are `pub(super)` only where the parent constructs/calls them; no public API changes. Parent is 913 lines; child is 182 formatted lines.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- Focused freshness test: `cargo test -p codex-router-cli --features keychain-test-support quota::quota_refresh_service::quota_refresh_helpers::freshness_tests::active_refresh_freshness_deadline_uses_the_shared_window_policy`: exit 0; 1 passed. The feature is required by the repository's compiled CLI acceptance guard.
- `cargo check -p codex-router-cli --locked`: exit 0, 16.04 seconds.
- `cargo clippy -p codex-router-cli --all-targets --locked --features keychain-test-support -- -D warnings`: exit 0, 29.88 seconds.
- Strict size checker: exit 1 with 23 remaining oversized files; quota refresh service is no longer listed.

A first focused command without the required feature correctly failed at the existing compiled CLI acceptance `compile_error!`; rerunning with `keychain-test-support` is the repository-required proof route. Linkage emitted the existing Apple `__eh_frame` warning only; no source warnings or command failure. No network/dependency/config/auth/production changes.
