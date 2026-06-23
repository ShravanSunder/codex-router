# Implementation Review After f75838c

Date: 2026-06-22
Branch: plan1a-quota-substrate-05bf755
Head: f75838c test: prove code review quota snapshot migration
Mode: focused implementation review after accepted review-finding fixes

## Verdict

ready

Reason: the final focused review found no P0-P2 findings and no P3 findings
worth flagging. The accepted quota selector, migration, credential mutation,
and OAuth refresh test-seam findings are closed in the current code at
`f75838c`.

## Findings

No accepted blocker, important, or follow-up findings remain from the focused
review.

## Accepted Findings Closed

- Legacy v3 `code_review` selector rows could survive forever because schema
  user_version stayed v3.
  - Disposition: fixed by schema v4 cleanup and polluted-v3 migration proof.
  - Proof: `tests::v3_migration_removes_legacy_code_review_selector_windows`.
- Credential mutation could leave stale status-only `code_review` selector rows.
  - Disposition: fixed by deleting credential-mutated selector rows before
    inserting selector-backed defaults only for response-backed route bands.
  - Proof: `tests::credential_mutation_invalidates_response_backed_alias_family_atomically`.
- CLI/proxy refresh-client injection constructors were production-visible test
  seams.
  - Disposition: fixed by moving fake refresh-client constructors behind
    `#[cfg(test)]`.
  - Proof: `cargo check -p codex-router-cli -p codex-router-proxy`.
- V2 migration proof did not prove `code_review` quota snapshots survive as
  status-only state.
  - Disposition: fixed by asserting the migrated `code_review` quota snapshot
    remains loadable while selector windows stay empty.
  - Proof: `tests::v2_migration_backfills_selector_windows_from_existing_quota_snapshots`.
- Production OAuth refresh endpoint/client-id environment overrides were unsafe.
  - Disposition: fixed by using production defaults without env override
    escape hatches; stale doc text corrected.
  - Proof: structural search receipt in
    `implementation-review-fix-2-receipt.md`.

## Review Proof

Focused reviewer: Sagan, agent `019ef06b-5191-7180-996b-768f23667df6`.

The focused reviewer independently checked these questions:

- `code_review` quota snapshots remain status-only while selector windows
  exclude `code_review` across v2/v3/v4 migration and credential mutation.
- OAuth refresh endpoint/client-id production overrides are absent.
- Refresh-client injection seams are test-only.
- No critical proof gap remains before PR wrap-up.

The reviewer reported all questions satisfied and reran the following proof set
at `f75838c`:

- `cargo test -p codex-router-state tests::quota_snapshot_upsert_keeps_code_review_out_of_selector_projection -- --exact`
- `cargo test -p codex-router-state tests::v2_migration_backfills_selector_windows_from_existing_quota_snapshots -- --exact`
- `cargo test -p codex-router-state tests::v3_migration_removes_legacy_code_review_selector_windows -- --exact`
- `cargo test -p codex-router-state tests::credential_mutation_invalidates_response_backed_alias_family_atomically -- --exact`
- `cargo test -p codex-router-cli tests::cli_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper -- --exact`
- `cargo test -p codex-router-proxy tests::proxy_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper -- --exact`
- `cargo test -p codex-router-proxy tests::loopback_http_streaming_adapter_returns_status_for_post_auth_proxy_rejections -- --exact`
- `cargo nextest run -p codex-router-state -p codex-router-auth -p codex-router-cli -p codex-router-proxy`
- `cargo nextest run --workspace`
- `cargo check -p codex-router-cli -p codex-router-proxy`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all --check`
- `git diff --check`

All passed at `f75838c`.

## Swarm Coverage

- Broad review lanes after `8b4cb9c` found the legacy v3 selector cleanup and
  production test-seam issues; both were accepted and fixed.
- Focused proof/contracts review after `e375159` found the v2 `code_review`
  snapshot preservation proof gap; it was accepted and fixed.
- Final focused review after `f75838c` found no remaining P0-P2 findings.

External counsel was not used because the user did not request Claude, Gemini,
`agy`, or Oracle for this focused review pass.

## Routing Follow-Through

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/implementation-review-after-f75838c.md`, reviewer `019ef06b-5191-7180-996b-768f23667df6`, proof commands listed above
recommended_next_workflow: shravan-dev-workflow:implementation-pr-wrapup
recommended_transition_reason: Implementation review is clean at `f75838c`; the next lifecycle gate is push, PR create/update, CI/review-thread freshness, and readiness proof.

## Remaining Boundaries

- Plan 1B cross-process quota refresh one-writer/lease behavior remains out of
  this Plan 1A checkpoint.
- Plan 2 router-owned interactive `login`, device-code, and OS keyring UX
  remains out of this checkpoint.
- Live quota cycling against real Codex accounts remains approval-gated
  operator proof.
