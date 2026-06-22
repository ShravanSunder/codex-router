# Plan Review After Revision

Reviewed commit: `e9e9e4a0ddf9e9c33d0bbbfdcc6f09079008f9c8`

Verdict: `needs_revision`

Do not send these plans to `implementation-execute-plan` yet. The revised
plans fixed meaningful structure from the prior review, but the review swarm
found remaining proof, ownership, sequencing, and security gaps that can let an
implementation satisfy the plan while missing required runtime behavior.

## Coverage

Parent coverage:

- `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`: 292 lines
- `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`: 355 lines
- `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`: 397 lines
- `docs/specs/2026-06-20-codex-router-greenfield-spec.md`: 497 lines
- `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`: 55 lines
- `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/details.md`: 82 lines before this update
- `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/events.jsonl`: 3 lines before this update

Reviewer lanes completed and closed:

- `spec-compliance`: Codex subagent, `needs_revision`
- `architecture-assumptions`: Codex subagent, `needs_revision`
- `testability-validation`: Codex subagent, `needs_revision`
- `security-reliability`: Codex subagent, `needs_revision`
- `execution-scope`: Codex subagent, `needs_revision`

## Accepted Blocker Findings

1. Hostile installed-smoke coverage is still missing.
   - Plan 1B's named installed smoke list omits the hostile no-token scenario
     even though the spec and umbrella require fail-closed hostile local request
     proof.
   - Required revision: add an exact smoke row for
     `installed_codex_hostile_no_token_smoke_keeps_upstream_empty`, including
     preflight, execution command, and expected zero-upstream-open observation.

2. Supported traffic proof is still classifier-only.
   - Row 1B-17 does not prove protocol forwarding for `/v1/models`,
     `/v1/memories/trace_summarize`, `/v1/responses/compact`, or WebSocket
     catalog header preservation.
   - Required revision: split route-classifier proof from route/protocol proof
     and add exact rows for each supported route and required header behavior.

3. Exact proof commands are not runnable or not exact as written.
   - Matrix rows use short test names with `cargo test -- --exact --list` and
     inexact or invalid `cargo nextest run` forms.
   - Required revision: use full libtest paths in every preflight and use an
     actually exact nextest form such as
     `cargo nextest run -p <pkg> -- <full::test::path> --exact` or an exact
     filterset.

4. Smoke proof is bundled instead of exact per scenario.
   - The wrapper script still dispatches by broad prefix and Plan 1B bundles
     several observations into broad smoke rows.
   - Required revision: make T12 own explicit scenario enumeration and add one
     exact matrix row per installed smoke scenario.

5. Credential resolver ownership is still architecturally ambiguous.
   - Plan 1A assumes an auth-owned unified resolver, but current manifests do
     not admit that dependency direction without explicit manifest changes.
   - Required revision: choose the resolver owner up front. If
     `codex-router-auth` owns it, manifest edits and proof rows must be
     explicit. If another crate owns it, move the rows there.

6. Replay-safe affinity substrate is still under-specified.
   - The plan promises replay-safe same-turn and continuation affinity, but it
     does not define envelope payload fields, binding scope, expiry or nonce
     rules, previous-response persistence, or WebSocket metadata-before-selection
     behavior.
   - Required revision: add a T9 design packet and proof rows for replay-scope
     validation, stale/wrong-key rejection, owner-unavailable continuation, and
     WebSocket selection after bounded first-frame metadata parse.

7. Dirty-tree isolation is only enforceable at the first checkpoint.
   - A1 has checkpoint commit discipline, but A2, B0, B1, and final closeout do
     not repeat the owned-path and same-hunk proof discipline.
   - Required revision: mirror A1's checkpoint receipt requirements at every
     later gate.

8. Fresh-worktree execution can drop untracked lifecycle state.
   - The preferred fresh worktree path does not say how to carry forward
     untracked `tmp/workflow-state` and `tmp/plan-workflows` artifacts.
   - Required revision: add a Gate 0 handoff step that either promotes required
     workflow artifacts or copies the exact tmp packet into the fresh worktree
     and records a carry-forward receipt.

9. Response-backed alias invalidation lacks family-atomic publication.
   - Plans require alias fan-out but not atomic publication or recovery across
     `responses`, `models`, `memories_trace_summarize`, and
     `responses_compact`.
   - Required revision: add one family publication rule and injected-failure
     proof that no mixed-generation response-backed alias state is observable.

10. Quota refresh one-writer proof does not cover cross-process owner/follower
    behavior or stale lease recovery.
    - T7 names a persisted fence or SQLite lease, but proof only covers
      overlapping manual/background refresh visibility.
    - Required revision: require persisted owner/follower semantics and a
      stale-owner recovery integration row using two refresh actors against the
      same router root.

## Accepted Important Findings

1. Local bearer lifecycle proof is incomplete and can reuse stale receipts.
   - Split row 1B-16 or attach exact rows for old-token HTTP rejection,
     missing/old-token WebSocket rejection before upstream open, and rotation
     closing old-generation WebSockets. Remove stale receipt reuse whenever T9
     or T12 touches proxy, WebSocket, or local-auth surfaces.

2. Plan 1A claims backend-neutral OAuth readiness without an acceptance proof.
   - Add a structural or compile-time row proving runtime entrypoints stop
     depending directly on `FileSecretStore`, or narrow the claim from OAuth
     readiness to resolver-only readiness.

3. Plan 1A does not prove selector-owned durable quota input.
   - Replace weak migration/partition rows with repository/API proof that the
     selector can read durable per-window selector input without depending on
     human status rows.

4. Plan 1B names `crates/codex-router-quota/src/*` without deciding quota
   runtime ownership.
   - Either remove that crate from the write surface if it stays helper-only or
     add an explicit boundary-extraction task that moves refresh runtime
     ownership there.

5. CLI help proof row 1B-22 is not executable on the current CLI surface.
   - Either change the row to commands that exist today or explicitly scope
     subcommand-help support into T11 with its own proof.

6. Mandatory lower-layer rows are missing or weak.
   - Add exact rows for turn-state envelope proof and profile write gating.
     Replace Plan 1A row 1A-00's `reports_package_name` proof with
     behavior-relevant account/quota/serve regression proof.

7. Audit JSONL append-failure policy is undefined.
   - Decide whether audit append failure is best-effort with surfaced local
     diagnostics or request-blocking, then add deterministic proof.

8. Workflow state freshness was stale.
   - `details.md` still referenced `bc3d5b8`; the reviewed plan commit is
     `e9e9e4a`. This receipt and the next transition event pin the current
     reviewed commit.

## Rejected Or Already Covered Findings

- Resolver bypass guard ownership was not missing; Plan 1A owns it in T4 and
  row 1A-14. The accepted resolver issue is crate ownership and manifest
  direction, not bypass omission.
- Response aliases versus `code_review` are no longer ambiguous; the revised
  plans keep `code_review` status-only unless a later spec promotes it.
- Audit JSONL allowlist/redaction coverage is owned; the remaining accepted gap
  is append-failure behavior.

## Parent Reduction

The findings dedupe into seven required revision themes:

1. Exact proof contract: full test paths, exact nextest syntax, executable CLI
   commands, and per-scenario smoke rows.
2. Runtime protocol coverage: supported routes and headers must be proven by
   protocol tests, not just classifier tests.
3. Credential and OAuth substrate: resolver ownership, backend-neutral runtime
   surfaces, and future keyring/OAuth readiness must be explicit.
4. Affinity and local auth: replay-safe envelope semantics, WebSocket metadata
   ordering, bearer lifecycle proof, and stale proof reuse rules.
5. Quota runtime and selection: selector-owned durable input, quota crate
   ownership, family-atomic alias publication, and cross-process refresh
   leases.
6. Execution hygiene: dirty-tree checkpoint proof at every gate and tmp
   lifecycle carry-forward for fresh worktrees.
7. Audit reliability: decide and prove audit append-failure behavior.

The smallest correct route is `shravan-dev-workflow:plan-creation-swarm`.
Implementation remains blocked until the plans are revised and re-reviewed.

phase_result: needs_revision
evidence: commit e9e9e4a0ddf9e9c33d0bbbfdcc6f09079008f9c8; reviewed plan files listed above; five read-only plan-review lanes completed and closed; this parent review receipt
recommended_next_workflow: shravan-dev-workflow:plan-creation-swarm
recommended_transition_reason: Revised plans still need plan changes around exact proof, installed smoke coverage, supported-route protocol proof, resolver ownership, affinity replay safety, selector projection, quota runtime boundary, dirty-tree receipts, tmp-state carry-forward, alias-family atomicity, cross-process quota refresh, local bearer lifecycle proof, and audit append-failure policy.
