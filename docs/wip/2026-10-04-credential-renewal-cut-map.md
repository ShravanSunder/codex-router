# Credential renewal outcome test cut map

Bounded auth-test pure reorganization under the owner continuation instruction. Credential semantics, encrypted-store fixtures, generation claims and failure classifications remain unchanged.

## Write set

- `crates/codex-router-auth/src/tests/credential_renewal_outcome_tests.rs`
- new `crates/codex-router-auth/src/tests/credential_renewal_outcome_tests/renewal_retry_tests.rs`

## Exact map

Before this cut, the outcome test file had 1350 lines. Keep imports, provider-pruning fixtures and tests through old line 931. Move old lines 932–1350 into the private child:

- `expired_claimed_successor_is_renewed_before_provider_egress`
- `typed_refresh_failures_persist_retry_or_reauth_without_reusing_ambiguous_token`
- `elapsed_retry_deadline_renews_even_when_ordinary_renewal_is_not_due`
- `confirmed_unspent_failure_retries_transient_sqlite_disposition_without_provider_reuse`
- `RejectingRefreshClient` and its implementation

The parent is 933 lines; child is 421 formatted lines. The existing `use super::*` scope and all encrypted-store/SQLite fixtures remain available. The parent adds a private path child declaration under the existing test module. No public auth API or credential behavior changes.

## Proof

- `cargo fmt --all -- --check`: exit 0.
- `cargo test -p codex-router-auth tests::credential_renewal_outcome_tests::renewal_retry_tests::`: exit 0; 4 passed, 0 failed, 75 filtered, 1.10 seconds.
- `cargo check -p codex-router-auth --locked`: exit 0, 4.45 seconds.
- `cargo clippy -p codex-router-auth --all-targets --locked -- -D warnings`: exit 0, 4.90 seconds.
- Strict size checker: exit 1 with 20 remaining oversized files; credential renewal outcome tests are no longer listed.

No credential schema/storage/auth/network/security changes, no dependency/config/CI/production changes.
