# Plan Review After `8fb965f`

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-review-swarm`
Reviewed commit: `8fb965fa80b0067f6bf062051b8c3726aa913511`
Verdict: `needs_revision`
Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

## Coverage

- Umbrella plan: `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`, 331 lines.
- Plan 1A: `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`, 463 lines.
- Plan 1B: `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`, 519 lines.
- Source spec at `HEAD`: `docs/specs/2026-06-20-codex-router-greenfield-spec.md`, 455 lines.
- Working-tree source spec remains dirty at 497 lines; the plan added source-freeze receipts but still needs an explicit account/quota base story.
- Workflow state: `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/details.md`, 129 lines; `events.jsonl`, 7 lines.

Read-only lanes completed and closed:

- `spec-compliance`: needs revision.
- `architecture-assumptions`: needs revision.
- `testability-validation`: needs revision.
- `security-reliability`: needs revision.
- `execution-scope`: needs revision.
- `structural-proof-commands`: needs revision.

## Parent Verdict

The revised plan is not ready for implementation. It fixed many prior review
findings, but still has base-state ambiguity and proof rows that can pass
without proving the intended safety property.

## Accepted Blockers

1. Fresh-worktree execution is ambiguous for `account` and `quota` CLI surfaces.
   - Evidence: `8fb965f` does not contain `crates/codex-router-cli/src/account.rs` or `crates/codex-router-cli/src/quota.rs`; `HEAD:crates/codex-router-cli/src/lib.rs` has no `account` command or non-live `quota status` parser.
   - Failure: an executor starting from a fresh worktree cannot know whether to preserve dirty product code, carry it forward, or create these surfaces from scratch.
   - Required revision: choose and document one base story. The safer plan route is to keep `8fb965f` as the reviewed base and make Plan 1A/T1/T3/T10 explicitly create the account/quota command surfaces, including parser/help wiring, module files, exact tests, and manifest dependencies. If pre-existing dirty product code is used instead, it must be promoted first or carried with path/checksum/line-count receipts.

2. Source-spec precommit auth/quota rotation is not planned or explicitly deferred.
   - Evidence: spec says rotation is allowed before response commit only for explicit account/auth/quota reasons; Plan 1B only proves next-request switching.
   - Failure: request N can hit explicit precommit quota/auth rejection and fail instead of selecting another eligible account, while the plan still passes.
   - Required revision: add Plan 1B T9 rows for explicit precommit auth/quota rotation and a negative row proving no router-created retry for transport, overload, timeout, DNS, reset, cancellation, or post-commit stream failures. If this is intentionally out of scope, route to spec review and defer it explicitly.

3. Resolver-bypass structural proof remains unsafe.
   - Evidence: `1B-27` filters by line substring allowlists; `1A-14a` forbids `FileSecretStore` while also saying file-backend construction may be allowlisted; `1A-14c` scans egress files and can both miss `access_token` and block legitimate resolver-output plumbing.
   - Failure: direct provider secret reads can pass by sitting on an allowlisted line, while harmless tests/bootstrap can fail.
   - Required revision: replace substring filters with explicit path/symbol allowlists, split provider-token bypass proof from backend/bootstrap proof, and scope selector-token proof to selector/account-decision DTO modules.

4. `1B-17ba` proves the wrong route.
   - Evidence: row claims `/v1/models` query preservation but points at an existing test that sends `/v1/responses?stream=true&cursor=abc`.
   - Failure: `/v1/models?...` query handling can regress while the row passes.
   - Required revision: add/rename an exact `/v1/models` query test such as `http_proxy_preserves_models_query_string_after_route_classification`.

5. WebSocket local-auth proof misses empty/wrong-token proxy rejection.
   - Evidence: current rows cover classifier, missing WS token, and old-token rotation, but not empty/wrong WS tokens before selection/upstream.
   - Failure: WS empty/wrong token can reach selection or upstream while HTTP/classifier rows pass.
   - Required revision: add a proxy-level WS row for empty, wrong, and old-token rejection with selector/upstream counters at zero.

6. Live proof can self-match stale runbook claims.
   - Evidence: row `1B-26` searches for generic live-proof phrases while `docs/testing/live-oauth-quota.md` contains approval-gating text and also stale live-run shaped fields.
   - Failure: closeout can claim `not-run: approval required` while docs still present an older live result as current proof.
   - Required revision: make `1B-26` validate the current gate block. Without approval, fail on current `live_oauth_quota_gate: run`, explicit approval, or result fields. With approval, require current dated approval and redacted output proof.

## Accepted Important Findings

1. Matrix preambles conflict with structural rows.
   - Exact-test stale guards should require exactly one listed test. Structural/search/docs/smoke rows should name expected match count or allowlist behavior per row.

2. Installed smoke can stale-green quota startup/status and transcript redaction.
   - Add a new/renamed smoke scenario or row-specific stale guard for startup-not-quota-blocked, redacted quota status table capture, and transcript allowlist/redaction parsing.

3. Plan 1B omits `codex-router-core` from required package gates while using a core local-auth row.
   - Add `cargo nextest run -p codex-router-core` or document an unchanged-core exception.

4. Audit append failure diagnostics need a production channel.
   - Name an API/channel such as `AuditFailureReporter` with production `tracing::warn!` or stderr behavior and test capture; structural proof must cover helper-swallowed errors too.

5. T9 can reintroduce provider tokens through turn-state wording.
   - State that turn-state never carries provider access, refresh, or bearer auth. If an upstream continuation value exists, name it separately and require resolver injection after affinity resolution.

6. Turn-state replay proof lacks restart semantics.
   - Add a proof row for replay across restart/new router instance. Either nonce state is durable or restart rotates/binds signing keys so old envelopes fail.

7. Backend-neutrality is under-specified around `file_backend::SecretStore`.
   - Name the neutral trait/factory location and forbid runtime entrypoints from importing the backend-neutral trait from a file-backend module.

8. Plan 1B checkpoint ownership is too prose-shaped.
   - Add a task-to-file/glob ownership table for T6 through T12, including manifests and shared-file hunk rules.

9. Docs proof row `1B-22` is manual but shaped like a command proof.
   - Make it explicitly manual with a reviewer checklist or split into deterministic positive/negative assertions.

10. Direct `codex-router-quota` dependency on secret-store remains an open architecture question.
    - Decide whether quota may depend on secret-store only for leases/factories, or whether all provider credential access must go through auth resolver/state.

## Rejected Or Deferred Findings

- None rejected. Some findings overlap and should be deduplicated during plan creation:
  resolver-bypass allowlist, selector token material, backend-neutrality, and turn-state provider-token wording are one boundary family.

## Required Next Revision Shape

Plan creation should revise the durable plan docs only, then checkpoint commit and rerun plan review. Do not implement product code yet.

Minimum revision checklist:

- [ ] Pick the account/quota source-base story and make fresh-worktree execution unambiguous.
- [ ] Add or explicitly defer precommit auth/quota rotation.
- [ ] Replace unsafe structural proof commands and allowlists.
- [ ] Tighten route, local-auth, live-proof, smoke, and docs proof rows.
- [ ] Add missing core gate and Plan 1B task-to-file ownership table.
- [ ] Clarify turn-state, replay restart behavior, audit diagnostics, and backend-neutral trait ownership.

phase_result: `needs_revision`

evidence: commit `8fb965fa80b0067f6bf062051b8c3726aa913511`; six read-only plan-review lanes completed and closed; parent verification commands inspected source/spec/code anchors; this review artifact.

recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`

recommended_transition_reason: Accepted blockers and important plan findings remain; implementation is blocked until plan creation revises the durable plan artifacts and another plan review passes.
