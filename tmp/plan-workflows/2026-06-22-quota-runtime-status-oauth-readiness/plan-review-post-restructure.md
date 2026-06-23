# Post-Restructure Plan Review Receipt

Date: 2026-06-22

Goal id: `2026-06-22-codex-router-quota-oauth-runtime`

Verdict: `needs_revision`

Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

Recommended transition reason: The post-restructure plans still diverge from the spec and are not executable with enough proof exactness, sequencing control, write-scope isolation, or security/reliability coverage to start implementation.

## Review Scope

Reviewed source documents end to end:

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md` (497 lines)
- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md` (208 lines)
- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md` (243 lines)
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md` (282 lines)
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-restructure-ledger.md` (66 lines)

Live repo evidence inspected:

- Current branch: `feature/initial-codex-router`
- Current plan commit: `bc3d5b8 docs: split quota runtime execution plan`
- Current remote push blocker: `origin https://github.com/shravan-agent/codex-router.git` returns repository not found.
- Current tree is dirty with pre-existing code/doc changes outside the plan commit.
- Current smoke wrapper dispatches ignored tests matching `installed_codex_`.
- Current ignored installed smoke tests are only:
  - `installed_codex_mock_smoke_exercises_generated_profile_token_and_websocket`
  - `installed_codex_hostile_no_token_smoke_keeps_upstream_empty`

Swarm lanes:

- spec-compliance: `needs_revision`
- architecture-assumptions: `needs_revision`
- testability-validation: `needs_revision`
- security-reliability: `needs_revision`
- execution-scope: `needs_revision`

Parent verdict: accept the review direction. Implementation should not start from these plans.

## Accepted Blocking Findings

### 1. OAuth/keyring requirement is not carried by the plan

The spec requires router-owned secure credential storage with OS keyring/Keychain as the required real backend and file storage only as fallback/dev/recovery. The umbrella plan defers this to a later Plan 2 and the current repo still has file-store-centered live behavior.

Given the user requirement that OAuth and multi-account setup with OAuth/device-code style login are part of this PR direction, the plan must be revised instead of silently treating this as out of scope.

Required plan repair:

- State exactly what this PR owns for OAuth/device-code login, import, router-owned credential state, and OS keyring/Keychain.
- If any portion remains Plan 2, write an explicit Plan 2 boundary and prove that Plan 1 cannot ship a UX that contradicts the final credential model.
- Make account setup commands explicit: login/add/import/list/status/select/disable/remove or the chosen equivalent.
- Include redacted UX and failure-mode proof for OAuth/device-code setup.

### 2. Same-turn and previous-response affinity are unowned

The spec treats `x-codex-turn-state` and `previous_response_id` routing as correctness-critical. The current umbrella defers the full turn-state envelope and the child plans do not carry a concrete implementation/proof row for same-turn account affinity.

Required plan repair:

- Add a plan-owned route-affinity contract for `x-codex-turn-state` and `previous_response_id`.
- Define the first request, follow-up request, missing metadata, malformed metadata, and account-ineligible cases.
- Prove WebSocket and HTTP/SSE paths cannot choose a different eligible account mid-turn.

### 3. Proof matrices are not exact or runnable enough

The spec requires exact proof rows with owner, command, expected observation, and freshness guard. The child plans still contain broad filters or placeholders:

- `plus CLI output tests`
- `plus quota refresh test`
- `named immediate test`
- `named scheduled test`
- `named shutdown test`
- broad filters such as `serve_`, `quota_status_`, `repository_backed_selector`, and `websocket_`
- zero-match named proof such as `quota_pace_runout_math`

Required plan repair:

- Add a `Proof owner` column.
- Replace broad filters and prose placeholders with exact test names or explicit new-test names.
- Add a preflight command for each named proof that must list exactly the expected test names before execution.
- Make smoke proof list exact installed smoke scenarios that actually exist or are explicitly added by the plan.

### 4. Smoke proof names scenarios that do not exist

Plan 1B names six installed smoke scenarios, but the current smoke wrapper only runs ignored tests matching `installed_codex_`, and only two ignored installed smoke tests currently exist. The plan cannot require scenario names without also adding those tests or changing the smoke harness.

Required plan repair:

- Either add the six named smoke tests as part of T12, or rename the smoke requirements to match the actual harness.
- Require the smoke script output to show each scenario/count individually.
- Keep hostile local-auth, HTTP/SSE auth injection/stripping, WebSocket first-frame routing, status redaction, and startup-not-blocked coverage as separate observations.

### 5. Dirty-tree isolation is missing

Gate 0 records `git status`, but does not isolate the already-dirty workspace. Since code changes already exist outside the plan commit, recording status alone is not enough to keep implementation receipts, review diffs, and checkpoint commits trustworthy.

Required plan repair:

- Require a fresh worktree/clean baseline before code execution, or
- require a dirty-path manifest that classifies every current path as `in-scope` or `out-of-scope`, and require child closeout to compare against that baseline.

### 6. Plan 1A/1B sequencing still has an unsafe escape hatch

The umbrella says Plan 1B must not start before Plan 1A validation and review pass, but also allows Plan 1B to be merged into a single PR with an explicit exception. That reopens the failure mode the split was meant to prevent.

Required plan repair:

- Delete the exception, or restate it as: a single PR is allowed only after a completed Plan 1A validation and implementation-review receipt exists inside that PR stack.
- Add explicit merge-gate subsections for A1, A2, B0, and B1 with required proof rows, diff state, review/workflow, and route-back rules.

### 7. Write surfaces and fan-out are not safe enough

Plan 1A says T2/T3 may fan out if scopes are disjoint, but the plan does not define task-local ownership and the live code shows those tasks converge on account import, identity derivation, credential metadata, and state repositories.

Plan 1B includes WebSocket-sensitive requirements, but `crates/codex-router-proxy/src/websocket.rs` is omitted from the write surface. Plan 1A also carries WebSocket credential proof while omitting the WebSocket file.

Required plan repair:

- Make T2 -> T3 serial unless a task-local write-surface table proves the intersection is empty.
- Add `crates/codex-router-proxy/src/websocket.rs` to write surfaces when WebSocket behavior is in scope, or explicitly prove the WebSocket behavior changes entirely through shared interfaces.

### 8. Unified credential resolver needs a structural bypass guard

The plan proves expired-token behavior, but does not prove quota refresh, HTTP/SSE, and WebSocket stop directly reading secrets outside the resolver. A partial implementation could pass expired-token tests while fresh-token paths still bypass the resolver.

Required plan repair:

- Define an auth-owned resolver contract used by quota refresh, HTTP/SSE, and WebSocket.
- Add a structural proof row that runtime `read_secret`/token-key reads do not remain in CLI/proxy request paths outside the resolver module.
- Include `Cargo.toml` and any quota/auth crate files required to make that boundary compile.

### 9. Audit JSONL redaction proof is missing

The plan covers redaction for CLI/status/stdout/stderr style surfaces, but not the audit JSONL sink itself. The spec makes audit JSONL a first-class secret sink with an allowlisted schema.

Required plan repair:

- Add a Plan 1A proof row that writes allowed HTTP, rejected HTTP, and rejected WebSocket flows to a temp audit sink.
- Assert no canary/token/raw label appears.
- Assert only allowlisted keys are present and rejected events retain required non-secret fields.

### 10. Credential mutation invalidation is too generic

The spec requires response-backed route-band alias replacement to stay consistent. Current code also has a separate `code_review` quota route/family. T3 should not merely say selector/status state is invalidated; it must say exactly what route-band state becomes stale/unknown after credential mutation or failed repair.

Required plan repair:

- Name the response-backed alias set from the spec: `responses`, `models`, `memories_trace_summarize`, and `responses_compact`.
- Decide and document whether `code_review` is also credential-mutation-scoped in the current implementation.
- Prove mutation failure and repair/reimport cannot leave stale positive quota rows eligible until a successful post-repair refresh.

### 11. Overlapping quota refresh needs a one-writer rule

The plan names concurrent refresh ambiguity but does not define serialization or generation fencing for manual refresh, immediate startup refresh, and scheduled refresh. Current persistence is atomic per account plus route band, not necessarily per full account refresh cycle.

Required plan repair:

- Define per-account quota-refresh serialization or cycle-generation fencing.
- State whether atomicity is per full account cycle or route-family group.
- Add a deterministic overlap test where manual and background refresh race, then assert only the winning cycle is visible across selector and status state.

### 12. Local bearer-token lifecycle proof cannot be deferred without receipt

The umbrella defers local bearer-token lifecycle unless already proven, but neither child plan includes a proof receipt. The spec requires local auth and rotation behavior, including WebSocket handling.

Required plan repair:

- Attach a current proof receipt if this is truly already proven, or
- pull the lifecycle into Plan 1B/T12 with exact proof rows for:
  - old-token HTTP rejection before account selection
  - missing/old-token WebSocket rejection before upstream open
  - rotation closing active old-generation WebSockets with redacted local close reason

## Accepted Important Findings

- The selector should not depend on human `quota_status_rows` as its preferred durable source. If status rows influence selection, define a selector-owned projection/DTO so T10 UX changes cannot alter selection semantics.
- The route support/proof rows need to cover supported Codex routes directly, including `models`, `memories_trace_summarize`, `responses_compact`, and unsupported/Realtimes fail-closed behavior.
- `memories_trace_summarize` needs forwarding/protocol proof, not just classifier coverage.
- The final terminal goal should stay PR-ready proof, not only local plan execution. Remote push/PR currently has a real blocker because the configured GitHub repository is not found.

## Rejected Or Deferred Lane Details

- The security lane named `code_review` as part of the invalidation set. Parent verification found `code_review` exists in current code as a separate quota route/family, while the spec's response-backed alias set is `responses`, `models`, `memories_trace_summarize`, and `responses_compact`. The revised plan should decide this explicitly rather than copy the lane wording as spec truth.
- No lane finding is rejected outright. The plan is not ready for implementation.

## Required Plan-Revision Checklist

- [ ] Reconcile OAuth/device-code login, multi-account setup UX, import behavior, and OS keyring/Keychain ownership with the spec.
- [ ] Add exact commands for login/add/import/list/status/select/disable/remove or the chosen account command vocabulary.
- [ ] Add same-turn and previous-response affinity requirements and proofs.
- [ ] Add `Proof owner` to every proof row.
- [ ] Replace placeholders and broad filters with exact test names.
- [ ] Add or rename installed smoke tests so the plan's smoke scenarios are real.
- [ ] Require dirty-tree isolation before implementation.
- [ ] Remove or constrain the single-PR Plan 1B escape hatch.
- [ ] Add executable merge-gate receipts for A1, A2, B0, and B1.
- [ ] Make T2/T3 serial unless task-local write surfaces are proven disjoint.
- [ ] Add WebSocket write-surface ownership or prove WebSocket behavior is satisfied through shared interfaces.
- [ ] Add unified resolver structural bypass proof.
- [ ] Add audit JSONL redaction/allowlist proof.
- [ ] Define route-band invalidation for response aliases and decide current `code_review` handling.
- [ ] Define quota-refresh one-writer serialization or generation fencing.
- [ ] Add local bearer-token lifecycle proof or attach a current proof receipt.
- [ ] Preserve PR-ready terminal proof and capture the current remote push blocker.

## Phase Receipt

phase_result: `needs_revision`

evidence:

- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`
- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`
- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-restructure-ledger.md`
- current repo grep/list evidence for route bands, smoke tests, and broad/placeholder proof rows
- five reviewer lane verdicts: all `needs_revision`

recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`

recommended_transition_reason: Plans need revision for spec alignment, proof exactness, smoke reality, sequencing, write-scope ownership, dirty-tree isolation, OAuth/keyring scope, and security/reliability proof before implementation can safely start.
