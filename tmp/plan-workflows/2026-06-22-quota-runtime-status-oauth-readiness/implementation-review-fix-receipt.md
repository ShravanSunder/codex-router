# Implementation Review Fix Receipt

Date: 2026-06-22
Branch: plan1a-quota-substrate-05bf755
Checkpoint base: 09631a2 feat: add durable selector quota input

## Accepted Review Findings Closed

- Durable selector input is now populated by production quota snapshot writes.
- v2-to-v3 migration backfills selector quota windows from existing quota snapshots.
- Credential mutation invalidates selector windows alongside quota snapshots.
- Weighted selector state is partitioned by route band, so models and responses do not bias each other.
- `SecretStore` is no longer defined in the file backend module; resolver code depends on the backend trait boundary.
- CLI/proxy credential runtime wrappers no longer wire `NoopCredentialRefreshClient`.
- CLI/proxy credential runtime wrappers use a Codex-compatible OpenAI OAuth refresh client, current wall-clock resolution, and shared refresh leases.
- Imported Codex OAuth bundles extract JWT `exp` claims when present because upstream Codex auth.json does not store a separate access-token expiry field.
- HTTP post-auth selection and provider-credential failures now emit redacted audit rejection events.
- HTTP post-auth selection and provider-credential failures now return stable HTTP responses instead of surfacing as generic connection failures.

## Proof

- `cargo nextest run -p codex-router-secret-store -p codex-router-auth -p codex-router-state`
  - 33 tests run: 33 passed, 0 skipped
- `cargo nextest run -p codex-router-proxy -p codex-router-cli`
  - 88 tests run: 88 passed, 0 skipped
- `cargo nextest run --workspace`
  - 148 tests run: 148 passed, 2 skipped
- `cargo clippy --workspace --all-targets -- -D warnings`
  - passed
- `cargo fmt --all --check`
  - passed

## Structural Proof

- `rg -n "file_backend::SecretStore|use codex_router_secret_store::file_backend::SecretStore" crates || true`
  - no matches
- `rg -n "FileSecretStore" crates/codex-router-cli/src/credential_runtime.rs crates/codex-router-proxy/src/credential_runtime.rs crates/codex-router-auth/src/resolver.rs || true`
  - no matches
- `rg -n "NoopCredentialRefreshClient" crates/codex-router-cli/src/credential_runtime.rs crates/codex-router-proxy/src/credential_runtime.rs crates/codex-router-auth/src/resolver.rs || true`
  - only definition/impl remains in `crates/codex-router-auth/src/resolver.rs`

## Remaining Scope

- Interactive router-owned `login` / device-code UX is still not part of this checkpoint.
- Live quota cycling with real accounts still needs an operator-run proof after OAuth/account setup is exercised against real Codex credentials.
