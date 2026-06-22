# Implementation Review Fix 2 Receipt

Date: 2026-06-22
Branch: plan1a-quota-substrate-05bf755
Checkpoint base: e9970a6 fix: close quota oauth review blockers

## Accepted Review Findings Closed

- Removed production OAuth refresh endpoint/client-id environment overrides from `OpenAiOAuthRefreshClient`.
- Deferred construction of the blocking `reqwest` client until refresh execution so resolver construction cannot panic at startup.
- Added wrapper-level CLI and proxy credential resolver tests proving expired OAuth bundles refresh through runtime adapters, not only through direct `RouterCredentialResolver` construction.
- Added streaming-loopback HTTP status proof for post-auth selector and credential failures.
- Kept `code_review` quota snapshots status-only for selector projection while still invalidating its quota snapshot on credential mutation.
- Made quota snapshot and selector projection writes transactional.
- Made v2-to-v3 selector-window migration transactional and excluded status-only `code_review` rows from selector backfill.
- Made credential mutation clear stale selector windows for routed bands before inserting the default ineligible window, so weekly windows cannot survive credential replacement.

## Scope Boundary

- Plan 1A credential resolver single-flight remains process-local and is proven by the auth resolver concurrent test plus CLI/proxy wrapper tests.
- Plan 1B still owns state-backed cross-process quota-refresh one-writer lease behavior in rows `1B-07a` and `1B-07b`.
- Plan 2 router-owned interactive `login` / device-code / keyring UX remains out of this checkpoint.
- Live quota cycling against real Codex accounts remains approval-gated operator proof.

## Proof

- `cargo test -p codex-router-state tests::credential_mutation_invalidates_response_backed_alias_family_atomically -- --exact`
  - 1 passed, 0 failed
- `cargo test -p codex-router-state tests::credential_mutation_invalidates_selector_windows_atomically -- --exact`
  - 1 passed, 0 failed
- `cargo test -p codex-router-state tests::quota_snapshot_upsert_keeps_code_review_out_of_selector_projection -- --exact`
  - 1 passed, 0 failed
- `cargo test -p codex-router-cli tests::cli_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper -- --exact`
  - 1 passed, 0 failed
- `cargo test -p codex-router-proxy tests::proxy_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper -- --exact`
  - 1 passed, 0 failed
- `cargo test -p codex-router-proxy tests::loopback_http_streaming_adapter_returns_status_for_post_auth_proxy_rejections -- --exact`
  - 1 passed, 0 failed
- `cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-cli -p codex-router-proxy`
  - 116 tests run: 116 passed, 0 skipped
- `cargo nextest run --workspace`
  - 152 tests run: 152 passed, 2 skipped
- `cargo clippy --workspace --all-targets -- -D warnings`
  - passed
- `cargo fmt --all --check`
  - passed
- `git diff --check`
  - passed

## Next Workflow

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/implementation-review-fix-2-receipt.md`, `cargo nextest run --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`
recommended_next_workflow: shravan-dev-workflow:implementation-review-swarm
recommended_transition_reason: Post-review fixes are implemented and locally verified; the next lifecycle gate is a fresh implementation review before PR wrap-up.
