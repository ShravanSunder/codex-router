# Plan Review After `918db95`

Date: 2026-06-22
Workflow: `shravan-dev-workflow:plan-review-swarm`
Goal id: `2026-06-22-codex-router-quota-oauth-runtime`
Reviewed commit: `918db95d06d4578ee604b097d49f9b2837a11368`
Verdict: `needs_revision`
Recommended next workflow: `shravan-dev-workflow:plan-creation-swarm`

## Coverage

- Umbrella plan:
  `docs/plans/2026-06-22-quota-runtime-status-oauth-readiness-plan.md`,
  352 lines, read in chunks `1-180`, `181-352`.
- Plan 1A:
  `docs/plans/2026-06-22-codex-router-plan-1a-credential-state-substrate.md`,
  520 lines, read in chunks `1-180`, `181-360`, `361-520`.
- Plan 1B:
  `docs/plans/2026-06-22-codex-router-plan-1b-quota-runtime-status-selection.md`,
  648 lines, read in chunks `1-180`, `181-360`, `361-540`, `541-648`.
- Prior review receipt:
  `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-review-after-734554d.md`,
  245 lines.
- Revision ledger:
  `tmp/plan-workflows/2026-06-22-quota-runtime-status-oauth-readiness/plan-revision-ledger.md`,
  144 lines.
- Workflow details:
  `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/details.md`.
- Transition log:
  `tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/events.jsonl`,
  11 valid JSONL events before this review transition.

Parent checks:

- `jq empty tmp/workflow-state/2026-06-22-codex-router-quota-oauth-runtime/events.jsonl` passed.
- Table column integrity over Plan 1A/1B passed with no mismatches.
- `git diff --check 734554d..918db95 -- docs/plans ...` passed.
- Targeted row search confirmed the post-`734554d` repairs exist:
  `1A-00d`, `1A-00e`, `1A-00f`, `1A-00g`, `1A-15a`, `1B-07b`,
  `1B-10a`, `1B-14b`, `1B-15`, `1B-17g`, `1B-23b`, `1B-23c`,
  `1B-26`, and `1B-27a`.
- `cargo test -p codex-router-cli tests::definitely_missing_plan_review_probe -- --exact --list`
  exits `0` with `0 tests`, re-confirming that raw exact-list exit code is not
  enough proof.
- Live-code anchor search confirmed the current repo still has the debt the
  plan is meant to remove: token-bearing proxy selector DTOs/direct secret reads
  in `http_sse.rs`, auth-owned `LiveQuotaClient`/`UsageResponse`, concrete
  `FileSecretStore` entrypoint use, and route/local-auth helper modules that
  affect Plan 1B proof.

## Swarm Lanes

- `spec-compliance`: `019eefa4-71aa-7011-8837-f78cf4dee605`,
  verdict `needs_revision`.
- `architecture-boundary`: `019eefa4-76c4-7383-b790-d0e3ffbfc20b`,
  verdict `needs_revision`.
- `security-reliability`: `019eefa4-7a63-73b3-a6e6-97067ad4b596`,
  verdict `needs_revision`.
- `execution-scope-validation-smoke`: `019eefa4-7e3f-70e3-b53d-dbf40d9eaa9c`,
  verdict `needs_revision`.

No external model lanes were requested. All lanes were read-only.

## Accepted Blockers

### B1. Plan 1A does not prove the exact-one preflight guard it depends on

Evidence:

- Prior blocker B1 required a real-test and missing-sentinel proof for the
  exact-one policy.
- Plan 1A states the policy in its proof contract and matrix preamble.
- Plan 1A A1/A2 gates do not require a dedicated exact-one helper proof row.

Failure scenario:

- Plan 1A can greenlight A1/A2 while the exact-one guard is broken or skipped,
  letting Plan 1B start from the same false-green pattern this review cycle is
  trying to prevent.

Required plan change:

- Add a Plan 1A matrix row analogous to `1B-23c`.
- Require it in Plan 1A closeout, preferably A2 or a shared Plan 1A validation
  gate.
- The row must run the helper against one known real Plan 1A test and one
  missing sentinel, recording stdout, exit code, expected names, and observed
  counts.

### B2. Plan 1B can reopen Plan 1A security surfaces without rerunning Plan 1A structural guards

Evidence:

- Plan 1A structural guards `1A-04b`, `1A-14c`, and `1A-15a` catch audit
  append swallowing, token-bearing selector DTOs, and selector dependency drift.
- Plan 1B T8/T9/T12 can edit `account_selection.rs`, selection modules,
  `http_sse.rs`, `websocket.rs`, and `server.rs`.
- Plan 1B final rows `1B-27`/`1B-27a` do not cover the full account-selection,
  selector, and audit-emission freshness scope.

Failure scenario:

- Plan 1A passes, then Plan 1B T9 reintroduces a token-carrying selector DTO,
  direct token-key read, or swallowed audit append failure in reopened proxy
  surfaces. Plan 1B can still close out because only local bearer proofs are
  explicitly freshness-rerun after T9/T12 proxy changes.

Required plan change:

- Add a Plan 1B freshness gate requiring current-checkout equivalents of
  `1A-04b`, `1A-14c`, and selector dependency-direction proof whenever
  T8/T9/T12 touch account-selection, selection, HTTP/SSE, WebSocket, server, or
  audit-emission paths.
- At minimum, extend `1B-27`/final structural proof or add a new row for
  account-selection/selector token carriers and audit append handling.

### B3. Auth/quota ownership still admits a forbidden Cargo cycle

Evidence:

- Plan 1B allows `codex-router-quota` to depend on auth for resolver/provider
  surfaces.
- The same ownership section also permits an auth-crate compatibility shim that
  delegates to `codex-router-quota`.

Failure scenario:

- One executor implements `quota -> auth`; another keeps `auth -> quota` as a
  delegating shim. The plan then describes an impossible Cargo graph, or two
  agents implement incompatible ownership while both claiming compliance.

Required plan change:

- Choose one acyclic graph. The preferred executable shape is:
  `codex-router-auth` owns credential resolver only;
  `codex-router-quota` owns provider quota DTO/client behavior;
  `codex-router-quota -> codex-router-auth`; never `auth -> quota`.
- Add structural proof using `cargo tree -p codex-router-auth -e normal`,
  `cargo tree -p codex-router-quota -e normal`, and package compile checks.

### B4. T9 replay and previous-response ownership remains a design placeholder

Evidence:

- Plan 1B says a T9 design packet must later name replay-state owner, nonce
  lifecycle, TTL, restart behavior, router-instance key behavior, and durable
  vs process-local semantics.
- Plan 1B rows `1B-13b` and `1B-14b` allow alternate designs instead of proving
  one chosen design.

Failure scenario:

- One executor chooses process-local replay cache plus restart fail-closed.
  Another chooses durable SQLite ownership. They require different write
  surfaces, manifests, recovery semantics, and tests, but both fit the current
  plan prose.

Required plan change:

- Pick the T9 owner before implementation:
  durable owner in `codex-router-state`, or explicit process-local/fail-closed
  owner outside state.
- Align write surfaces and rows `1B-13b`/`1B-14b` to one design only.

### B5. Dirty-tree checkpoint proof is still not mechanically auditable

Evidence:

- The umbrella requires owned-path-only checkpoint receipts.
- Child task ownership still includes open-ended buckets such as adjacent
  auth/quota service files, proxy audit/status surfaces, refresh-worker/serve
  startup manifest edges, local-auth route support files, and closeout-only
  docs/runbook receipts.

Failure scenario:

- A checkpoint touches an unnamed file and still claims compliance because the
  reviewer cannot mechanically compare `git show --name-only <checkpoint>` to a
  closed allowlist.

Required plan change:

- Replace open-ended buckets with explicit paths or explicit wildcards already
  listed in `Write Surfaces`.
- If a new file may be needed, require a pre-task plan amendment before that
  task starts.

### B6. Merge Gate A1 requires resolver proof before T4 owns the resolver

Evidence:

- Plan 1A row `1A-06b` proves the credential resolver reads only the active
  credential bundle.
- Row `1A-06b` is included in A1 rows `1A-00 through 1A-06b`.
- T4 owns resolver call-site migration and starts after A1.
- Reviewed base `8fb965f` has no `router_credentials.rs`, so this cannot be
  satisfied by pre-existing resolver implementation.

Failure scenario:

- A1 can only pass by doing T4 work early or by overclaiming resolver readiness.

Required plan change:

- Move `1A-06b` to A2, or split it into:
  an A1 row proving active-generation publication semantics, and an A2 row
  proving the T4 resolver consumes only the active generation.

### B7. Required rows still use prose instead of exact execution commands

Evidence:

- Plan 1B proof contract requires exact execution commands.
- Rows `1B-23c`, `1B-26`, and `1B-27a` contain prose execution instructions
  rather than copy-pasteable commands or a named helper invocation.

Failure scenario:

- Different executors can implement different preflight-helper behavior, live
  gate parsing, or backend-construction grouping while all claiming the same
  proof row passed.

Required plan change:

- Replace prose cells with exact shell commands or a named helper script path.
- The `1B-23c` command must fail nonzero for the missing sentinel even though
  raw `cargo test -- --list` exits `0`.
- The `1B-26` command must parse only the current gate block and fail on
  missing/duplicate blocks or forbidden approval/result keys when approval is
  absent.
- The `1B-27a` command must produce deterministic grouped output for
  production entrypoints, factory files, test-only reporting, and compile check.

## Accepted Important Findings

### I1. Activation profile rows do not assert the exact local-auth header mapping

Evidence:

- Rows `1A-00d`, `1A-00f`, and `1A-00g` require `CODEX_ROUTER_TOKEN`, but do
  not require the exact TOML header mapping.

Failure scenario:

- The emitted profile can use the wrong header key while still satisfying the
  row, causing installed Codex to send the bearer in the wrong place.

Required plan change:

- Require the exact mapping:
  `"X-Codex-Router-Token" = "CODEX_ROUTER_TOKEN"`.
- Make printed, dry-run, and approved-write profile text compare against the
  same expected stanza.

### I2. Plan 1B write surfaces omit route and local-auth helper owners

Evidence:

- Live route classification is centralized in
  `crates/codex-router-proxy/src/routes.rs`.
- Local token validation is owned through
  `crates/codex-router-proxy/src/local_auth.rs`.
- Plan 1B T9 references vague local-auth route support files and omits these
  exact files from its closed write surfaces and task-owned file table.

Failure scenario:

- An implementer fixes unsupported routes or local-token behavior in higher
  layers while the shared classifier/auth helper remains stale, causing HTTP
  and WebSocket divergence.

Required plan change:

- Add `crates/codex-router-proxy/src/routes.rs` and
  `crates/codex-router-proxy/src/local_auth.rs` to the Plan 1B write surfaces
  and T9 ownership table.
- Require rerunning `1B-16aa`, `1B-16ab`, `1B-16bd`, `1B-17a`, `1B-17f`, and
  `1B-17g` when those files change.

### I3. Row `1B-07b` does not scan every T7 refresh entrypoint

Evidence:

- T7 ownership includes `crates/codex-router-cli/src/lib.rs`.
- Row `1B-07b` scans quota source and `crates/codex-router-cli/src/quota.rs`
  only.

Failure scenario:

- Serve startup glue can route through the process-local `RefreshLeaseManager`
  while `1B-07b` still passes.

Required plan change:

- Expand `1B-07b` to all T7 refresh entrypoints, including CLI serve startup
  glue, or explicitly prove those entrypoints cannot own quota-cycle
  coordination.

### I4. T7 lease owner is still a live choice without aligned ownership

Evidence:

- T7 allows either state-backed SQLite lease or quota-owned persisted
  generation fence.
- T7 task ownership does not explicitly add state repository/sqlite ownership
  for the SQLite lease choice.

Failure scenario:

- An executor chooses the SQLite lease and must edit state surfaces that are not
  clearly T7-owned, or chooses quota-owned fencing while leaving state-backed
  wording in proof rows.

Required plan change:

- Pick quota-owned persisted fence, or add explicit state repo/sqlite ownership
  to T7.
- Make `1B-07a`/`1B-07b` name the chosen owner.

## State Artifact Fix Applied By Orchestrator

The execution-scope lane found that the workflow details freshness guard still
named `734554d` for the plan-matrix gate. The official event log already had
the `918db95` plan-creation event, so the parent orchestrator will update
`details.md` with this review transition.

## Rejected Or Deferred Candidate Findings

- Broad concern that the post-`734554d` repairs were absent is rejected. Parent
  search and full-file review confirmed the requested rows and clauses were
  added.
- Live OAuth/device-code execution remains deferred/approval-gated. The
  accepted issue is command exactness and Plan 1/Plan 2 wording, not permission
  to run live proof now.
- Current product-code debt, such as token-bearing selector DTOs and auth-owned
  quota client types, is not itself a plan-review failure. It is accepted only
  where the plan does not make the required future change unambiguous and
  provable.

## Route Decision

phase_result: `needs_revision`

evidence:

- this review artifact
- four read-only lane receipts listed above
- parent verification commands described in coverage
- current revised plans at reviewed commit
  `918db95d06d4578ee604b097d49f9b2837a11368`

recommended_next_workflow: `shravan-dev-workflow:plan-creation-swarm`

recommended_transition_reason: The revised plan addressed the previous review
cycle but still has accepted blocker-level issues in exact proof commands,
Plan 1A/1B gate ordering, dirty-tree auditability, auth/quota dependency
direction, T9 ownership, and structural proof freshness. Code implementation
must not begin until those issues are folded into durable plan docs and reviewed.
