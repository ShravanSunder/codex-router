# Plan Review After `734554d`

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-review-swarm`
Goal id: `2026-06-22-codex-router-quota-oauth-runtime`
Reviewed commit: `734554df3329682d40d8495ca091e6ca6e326cf5`
Verdict: `needs_revision`
Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

## Coverage

- Spec: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, 497 lines, read in chunks `1-180`, `181-360`, `361-497`.
- Umbrella: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`, 342 lines, read in chunks `1-180`, `181-342`.
- Plan 1A: `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`, 492 lines, read in chunks `1-180`, `181-360`, `361-492`.
- Plan 1B: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`, 587 lines, read in chunks `1-180`, `181-360`, `361-540`, `541-587`.
- Revision ledger: `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`, 120 lines.
- Parent checks:
  - table column integrity over Plan 1A/1B: no mismatches.
  - stale-pattern search over plans/ledger: no matches.
  - `cargo test -p codex-router-cli tests::definitely_missing_plan_review_probe -- --exact --list` exits `0` with zero tests, validating the preflight false-green issue.
  - current `docs/testing/live-oauth-quota.md` has generic `not-run: approval required` text and a separate current gate block.
  - current backend-construction proof command for row `1B-27a` fails and duplicates raw `rg` output because it line-filters search output instead of using path/scope-aware policy.

## Swarm Lanes

- `spec-compliance`: `019eef7a-3236-76e0-bfd5-754a050f8574`, verdict `needs_revision`.
- `architecture-assumptions`: `019eef7a-3514-75d2-af79-2c14af9d2c78`, verdict `needs_revision`.
- `testability-validation`: `019eef7a-3806-7121-9633-c0f14bb1a410`, verdict `needs_revision`.
- `security-reliability`: `019eef7a-3a76-7a10-81ff-238f5f871590`, verdict `needs_revision`.
- `execution-scope`: `019eef7a-3cc2-7572-8eb9-6df645644dbc`, verdict `needs_revision`.

No external model lanes were requested. All lanes were read-only.

## Accepted Blockers

### B1. Exact-test preflights can false-green on missing tests

Evidence:

- Plan 1A and Plan 1B say exact-test preflights fail if the exact test is missing, duplicated, or stale.
- The matrix uses plain `cargo test ... -- --exact --list`.
- Parent and testability lane both verified that a missing exact test can return exit `0` with `0 tests`.

Failure scenario:

- An executor records the preflight as passed while the proof test was never added.

Required plan change:

- Add a shared exact-test preflight helper or inline command pattern that fails unless exactly one expected test is listed.
- Apply it to normal exact tests and ignored smoke tests.
- Add proof that the helper passes for one known real test and fails for a missing sentinel.

### B2. Installed-Codex generated-profile/token/WebSocket smoke is named but not final-gated

Evidence:

- Spec smoke proof requires installed Codex through a router profile, temp Codex home/profile fixture, token injection without printing, HTTP/SSE and WebSocket modes when enabled, and transcript assertions.
- Plan 1B names `installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket`.
- Plan 1B matrix and final closeout mandate startup/status, transcript redaction, hostile no-token, and wrapper rows, but no exact row for the generated-profile/token/WebSocket smoke.
- Current test support already has the broad installed smoke test name, so omitting it is not hypothetical.

Failure scenario:

- Final closeout can pass without proving the actual generated profile/token/WebSocket installed-Codex path.

Required plan change:

- Add a dedicated exact ignored matrix row for `installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket`.
- Update the final closeout gate and wrapper row to require all named smoke scenarios and exact counts.

### B3. Profile activation/write proof is under-matrixed and partly deferred

Evidence:

- Spec requires read-only activation output: profile text, shell-safe token export command, doctor check without printing token, and dry-run profile write preview.
- Spec integration proof requires profile print/profile dry-run not mutating `~/.codex`, and profile apply requiring explicit approval and writing only the named profile file.
- Umbrella defers profile write apply with approval out of Plan 1A/1B.
- Plan 1A only has a single profile-write guardrail row.

Failure scenario:

- Plan 1 can pass while users still lack proven activation commands required to run Codex through the router safely.

Required plan change:

- Move activation/profile proof out of the deferred full-spec rows.
- Add explicit Plan 1A or Plan 1B rows for profile print, token export redaction, doctor redaction, dry-run no-mutation, and approved write to a temp named profile only.

### B4. Durable selector projection ownership is ambiguous and dependency-unsafe

Evidence:

- Plan 1A says `codex-router-state` owns durable account/quota/selector projection persistence and selector consumes a selector-owned durable projection table/API.
- Plan 1B assigns T8 to selection plus state files but omits `crates/codex-router-selection/Cargo.toml`.
- Current selection depends on quota, not state; current repository-backed selection is proxy-owned.

Failure scenario:

- One executor makes `selection` depend on `state` outside write surfaces; another keeps repository-backed selection in proxy. Both can claim plan compliance, but they produce different dependency boundaries.

Required plan change:

- Pick the boundary explicitly. Preferred: `selection` remains pure and consumes state-free selector DTOs; proxy or a named adapter reads `codex-router-state` projection and maps to selection DTOs.
- If selection reads state directly, add `crates/codex-router-selection/Cargo.toml` to write surfaces and add acyclic dependency proof.
- Add a dependency-direction/structural proof row naming the only owner of repository-backed selector projection.

## Accepted Important Findings

### I1. Quota provider-client ownership conflicts across Plan 1A/1B

Evidence:

- Plan 1B says `codex-router-quota` owns provider clients.
- Plan 1A includes `codex-router-auth/src/live_quota.rs` and `quota_client.rs`.
- Current CLI imports provider quota DTO/client types from auth, and auth owns the blocking quota client.

Required plan change:

- State whether provider quota DTO/client moves to `codex-router-quota` in Plan 1A/A2, or revise Plan 1B so quota owns orchestration while auth owns provider quota transport behind an explicit trait.
- Add write surfaces and structural proof for the chosen ownership.

### I2. Quota lease boundary can accidentally use the process-local secret-store lease

Evidence:

- Plan 1B permits quota to depend on secret-store for refresh leases.
- T7 requires cross-process persisted cycle-generation fencing or SQLite lease, not an in-memory mutex.
- Current `RefreshLeaseManager` is process-local.

Required plan change:

- Forbid using current process-local `RefreshLeaseManager` for quota one-writer behavior unless it is upgraded to persisted semantics.
- Name the owner: state-backed SQLite lease or quota-owned persisted generation fence.
- Add structural proof that quota does not use process-local `RefreshLeaseManager` for the Plan 1B quota cycle.

### I3. Selector/token DTO proof can miss the existing proxy token carrier

Evidence:

- Plan 1A row `1A-14c` scans selection and new `account_selection.rs`.
- Current `SelectedUpstreamAccount` and `UpstreamAccountSelector` live in `http_sse.rs` and carry `upstream_auth_token`.

Required plan change:

- Require deletion/redefinition of the old proxy token-bearing selection DTOs.
- Expand structural proof to include `http_sse.rs`, `websocket.rs`, `account_selection.rs`, and selection modules with a precise resolver-output allowlist.

### I4. Proxy module write ownership is incomplete

Evidence:

- Plan 1A names new proxy modules `account_selection.rs` and `secret_store_factory.rs`, but not `crates/codex-router-proxy/src/lib.rs`, which must declare/export them.
- Plan 1B task table owns `account_selection.rs`, but top-level write surfaces omit it.

Required plan change:

- Add `crates/codex-router-proxy/src/lib.rs` to Plan 1A write surfaces and T4 ownership, limited to module declarations/re-exports.
- In Plan 1B, add `account_selection.rs` to write surfaces or state T9 may only extend the Plan 1A-created module.
- Add shared-file hunk fingerprint and module-declaration-only receipt requirements.

### I5. Previous-response affinity lacks restart/unknown-owner contract

Evidence:

- Spec requires previous-response affinity to prefer the owning account or fail clearly rather than silently replaying state on another account.
- Plan 1B says previous-response ownership persists after commit but does not define durable owner storage, unknown-owner behavior after restart, or a two-instance/restart proof for `previous_response_id`.
- Current base has in-memory affinity state.

Required plan change:

- Add a T9 decision: previous-response ownership is SQLite-durable, or unknown ownership after restart fails before weighted selection.
- Add state/repository write surfaces if durable.
- Add a proof row for committed previous-response owner survival across restart/new router instance or explicit unknown-owner fail-closed behavior.
- Require that row in B1.

### I6. Unsupported WebSocket routes are not explicitly fail-closed

Evidence:

- Spec requires unsupported routes, including Realtime/WebRTC and unknown paths, to fail closed before account selection.
- Row `1B-17f` only names HTTP rejection.

Required plan change:

- Add or extend proof for authenticated WebSocket upgrade to unsupported paths rejecting before selection/upstream open.

### I7. WebSocket first-frame proof should cover non-affinity routing

Evidence:

- Spec requires the router to wait for the first `response.create`, read bounded metadata, select account, open upstream, and forward the exact first frame unchanged.
- Row `1B-15` is framed around continuation/affinity metadata.

Required plan change:

- Expand first-frame proof to assert zero upstream open before first `response.create`, byte-exact first-frame forwarding, and bounded metadata-only parsing for both affinity and non-affinity first frames.

### I8. Live OAuth gate command can self-match stale prose

Evidence:

- Row `1B-26` searches the whole runbook for generic phrases such as `not-run: approval required`.
- The runbook contains generic approval-boundary text and a separate current gate result block.

Required plan change:

- Make `1B-26` parse only the `## Current Gate Result` fenced block.
- For no approval, require exactly `live_oauth_quota_gate: not-run` plus the changed-revision reason.
- For approval, require a dated/current approval receipt and redacted result block.

### I9. Backend-construction structural proof uses fragile line filtering

Evidence:

- Row `1B-27a` claims path/test allowlisting, but pipes raw `rg` output through line-based `rg -v` filters for `src/secret_store_factory.rs`, `#[cfg(test)]`, and `mod tests`.
- This does not prove that occurrences are actually inside test modules and can false-fail valid test-only occurrences.

Required plan change:

- Replace row `1B-27a` with scope-aware checks:
  - production entrypoint files have zero concrete file-backend matches,
  - named factory files contain the expected construction,
  - test modules are checked separately or by exact test-only paths.

## Rejected Or Deferred Candidate Findings

- Security lane's concern that secret storage, resolver bypass, partial credential writes, token egress allowlisting, audit diagnostics, local bearer rotation, turn-state replay, quota one-writer, and unsupported routes are broadly missing was rejected as overbroad. The plan covers these, but the accepted targeted findings above still require revision.
- Execution-scope lane's dirty-worktree concern was rejected as a new blocker. The umbrella and child Gate 0 rules are sufficient once the accepted write-surface/module ownership gaps are fixed.
- The architecture lane's note that parent/spec/ledger paths were missing is rejected as lane-local navigation error; the parent verified all required paths and coverage.

## Route Decision

phase_result: `needs_revision`

evidence:

- this review artifact
- lane receipts listed above
- parent verification commands described in coverage
- current plans at reviewed commit `734554df3329682d40d8495ca091e6ca6e326cf5`

recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`

recommended_transition_reason: The revised plan still has accepted blocker and important plan issues that must be folded into durable plan docs before any implementation begins.
