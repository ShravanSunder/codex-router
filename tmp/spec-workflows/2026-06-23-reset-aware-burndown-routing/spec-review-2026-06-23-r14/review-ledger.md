# R14 Spec Review Ledger

Date: 2026-06-23
Reviewed baseline: `f104ff9`
Spec: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`

## Coverage

Parent verified the target spec at 1788 lines and read it in chunks:

- 1-320
- 321-640
- 641-960
- 961-1280
- 1281-1600
- 1601-1788

Lane receipts:

- `lanes/whole-spec-coverage-planning.md`
- `lanes/security-harness.md`
- `lanes/spec-difference-validation.md`

## Lanes Run

| Lane | Reviewer | Status |
| --- | --- | --- |
| whole-spec-coverage + planning-readiness | Galileo (`019ef5f4-5ad4-73f2-ad66-78185f75bcf6`) | needs_revision |
| security-threat-model + harness-fit | Archimedes (`019ef5f4-8e16-7000-9cee-10a2548bc9fa`) | needs_revision |
| spec-difference + validation-and-testability | Socrates (`019ef5f4-bc9a-7903-a5dd-c4b53fde6ca8`) | needs_revision |

## Verdict

needs_revision

Do not route to `plan-creation-swarm` yet. R14 found accepted blockers that
would force the plan to reinterpret core requirements, especially local auth,
generated Codex profile compatibility, WebSocket preselection, unknown fallback
semantics, and route-level result contracts.

## What Held

- Startup must remain fast and must not block on live quota refresh.
- Routing must use last-known SQLite quota while background probes refresh the
  truth.
- Failed, no-auth, quota-empty, or probe-failed accounts must not be selected.
- Weekly quota is the long-horizon limiter and must be weighted more strongly
  than 5h quota.
- The status UI should show at most account-level 5h/weekly quota, reset time,
  burn pace, and next-use/routing meaning, with Unicode bars in normal output.
- Installed Codex WebSocket proof remains mandatory for any real completion
  claim.

## Accepted Blockers

### B1: Scope Trace Is Incomplete

The spec top-level requirements describe quota routing and status behavior, but
later sections also require affinity-secret cutover, local-auth carrier cutover,
generated Codex profile changes, WebSocket first-frame restrictions, and smoke
artifact redaction. These need explicit top-level requirement rows or a split
out of this goal.

Route: spec revision.

### B2: Local Auth and Generated Profile Contract Contradict the Proven Path

The spec requires `X-Codex-Router-Token` via generated `env_http_headers` and
forbids `env_key`/`Authorization`. Current code and tests still generate or
accept `env_key`/`Authorization`.

Route: spec revision must decide the target from installed Codex reality, then
implementation can hard-cut to the chosen contract.

### B3: WebSocket Preselection Allowlist Conflicts With Current Runtime Shape

The spec says WebSocket routing may read only minimal top-level routing metadata
before selection. Current runtime code reads direct payload fields such as
`model`, `input`, and `stream` in one WebSocket branch.

Route: spec revision must either allow the smallest real installed-Codex frame
surface or explicitly delete the compatibility branch and update smoke proof.

### B4: Unknown-Quota Fallback Pool Is Not a First-Class Contract Yet

The spec says unknown quota should enter a fallback/probe path and only become
usable after evidence. Current selection semantics model unknown as
`ProbeRequired` and do not expose a first-class fallback selected pool.

Route: spec revision must define fallback pool state, probe transitions,
ordering, cooldown, pinning, and proof rows.

### B5: Route-Level Result Envelope Is Not Canonical

The spec uses overlapping fields and concepts such as `preferred_next`,
`preferred_next_account_id`, `route_result`, and unsupported route-band payloads
without one canonical envelope.

Route: spec revision must define one route-level result schema for selection,
CLI JSON, proxy behavior, and tests.

## Accepted Important Findings

- Refresh staleness needs an owner, formula, and transition rules.
- Status/JSON DTO ownership needs a precise selection-owned machine shape.
- Smoke transcripts need an allowlisted redaction schema and proof.
- Affinity secret cutover needs an explicit order across schema, repository,
  hashing owner, HTTP/SSE, WebSocket, tests, and raw-key deletion.

## Next Workflow

`shravan-dev-workflow:spec-creation-swarm`

The next step is another spec revision, not implementation planning.

```text
phase_result: needs_revision
evidence: tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/review-ledger.md; tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/lanes/whole-spec-coverage-planning.md; tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/lanes/security-harness.md; tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r14/lanes/spec-difference-validation.md
recommended_next_workflow: shravan-dev-workflow:spec-creation-swarm
recommended_transition_reason: R14 found accepted blockers in scope trace, local-auth/profile compatibility, WebSocket preselection, unknown fallback semantics, and route-result contracts, so plan creation remains blocked.
```
