# Plan 1A T3 Receipt: Fail-Closed Credential Writes

## Scope

- Added router-owned bundled account credential payloads in `codex-router-secret-store`.
- Added `active_credential_generation` account metadata in SQLite schema version 2.
- Changed `account import-codex-auth` to write a disabled pending account, persist one bundled credential generation, then atomically activate the generation and invalidate quota snapshots.
- Replaced runtime import assertions for legacy access/refresh token keys with bundled credential assertions. The legacy key helpers remain available for migration/import compatibility but are not the post-T3 runtime truth.

## Red Evidence

Initial exact T3 proof rows failed before implementation:

```text
cargo test -p codex-router-cli tests::account_import_codex_auth_partial_secret_write_disables_account_until_repair -- --exact
E0432 missing account_credential_bundle_key / AccountCredentialBundle / AccountImportRequest / import_codex_auth_from_request

cargo test -p codex-router-cli tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation -- --exact
E0432/E0599 missing bundle and active credential generation APIs

cargo test -p codex-router-state tests::credential_mutation_invalidates_response_backed_alias_family_atomically -- --exact
E0599 missing activate_account_credential_generation_and_invalidate_quota and active_credential_generation
```

One implementation-order failure was caught and fixed:

```text
tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation
left: Some(1)
right: Some(2)
```

Cause: import disabled the account before computing the next credential generation. Fix: compute next generation from existing account state before writing disabled pending metadata.

## Green Evidence

Exact T3 rows:

```text
cargo test -p codex-router-cli tests::account_import_codex_auth_partial_secret_write_disables_account_until_repair -- --exact
1 passed; 0 failed

cargo test -p codex-router-cli tests::account_import_codex_auth_invalidates_quota_snapshot_on_credential_mutation -- --exact
1 passed; 0 failed

cargo test -p codex-router-state tests::credential_mutation_invalidates_response_backed_alias_family_atomically -- --exact
1 passed; 0 failed
```

Touched-crate gates:

```text
cargo fmt --all --check
exit 0

cargo nextest run -p codex-router-cli -p codex-router-state -p codex-router-secret-store
53 tests run: 53 passed, 0 skipped

cargo clippy -p codex-router-cli -p codex-router-state -p codex-router-secret-store --all-targets -- -D warnings
exit 0

cargo check -p codex-router-cli -p codex-router-state -p codex-router-secret-store
exit 0

git diff --check
exit 0
```

## Notes

- No provider network calls or live OAuth proof were run.
- No subagents were used for this slice.
- The account remains disabled with no active credential generation if the bundle write fails.
- Credential mutation writes stale quota markers for `responses`, `models`, `memories_trace_summarize`, `responses_compact`, and `code_review`.
