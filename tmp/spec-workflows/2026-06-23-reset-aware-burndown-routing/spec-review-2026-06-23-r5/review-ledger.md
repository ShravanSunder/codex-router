# Reset-Aware Burn-Down Routing Spec Review Ledger R5

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `a7dd754cb9ac6016fd767ec2dcb1515af0fed696`
Review worktree: `/tmp/codex-router-r5-review.zGGh9D`
Coverage: 1014 lines, read by parent in chunks 1-180, 181-360, 361-540, 541-720, 721-900, 901-1014
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Lagrange | answered | needs revision |
| requirements-testability + validation-and-testability + planning-readiness | Chandrasekhar | answered | needs revision |
| contract-and-scope + architecture-boundaries + spec-difference | Sagan | answered | needs revision |
| security-threat-model + WebSocket/protocol | Wegener | answered | needs revision |
| adversarial-crux + guardrail-codification + UX/status | Carver | answered | needs revision |

## What Held

- R4 WebSocket path classification, first-frame bounds, safe labels, and
  redaction canary expectations are materially present.
- The selection ownership split remains coherent:
  `codex-router-selection::burn_down` owns pure assessment, proxy owns
  runtime-exact selection and affinity, state owns persisted rows, and CLI owns
  formatting.
- The public 5h plus weekly v1 shape is clear enough to preserve the user's
  quota-table mental model.

## Accepted Findings

### R5-A1. Unknown-only fallback is routable but not publicly mapped

Severity: blocker

Required revision:

- Add explicit public mapping for `selected_pool=unknown`.
- The preferred unknown fallback must not be described as healthy quota.
- Add proof for preferred and non-preferred all-unknown rows.

### R5-A2. Salvage tie ordering is not deterministic enough

Severity: blocker

Required revision:

- Replace `earlier near-reset salvage` with an exact sortable tie key.
- Preserve equality between ordered `weighted_candidates`, neutral
  `preferred_next`, and empty-state `WeightedDeficitSelector`.

### R5-A3. Codex-through-router e2e acceptance is under-specified

Severity: blocker

Required revision:

- Define the minimum e2e acceptance contract.
- The local required gate is installed Codex CLI plus generated router profile,
  served local router, mock upstream, HTTP/SSE and WebSocket transport, multiple
  persisted accounts, reset-aware choice, status/reason agreement, selected
  account pinning, and redacted transcript artifacts.
- Live OAuth/quota cycling remains a separate approval-gated proof layer.

### R5-A4. WebSocket preselection failure proof needs a closed matrix

Severity: important

Required revision:

- Expand proof from generic malformed/unsupported/auth-failed paths to a
  failure-mode matrix.
- Include local auth, unsupported path, non-text/non-JSON, wrong type,
  oversized frame, timed-out frame, malformed affinity, and owner-resolution
  failures.
- Require zero selector advance, zero credential resolver call, zero upstream
  auth injection, and zero upstream open for each row.

### R5-A5. Previous-response affinity extraction is not exact

Severity: blocker

Required revision:

- Specify top-level `previous_response_id` extraction for HTTP/SSE JSON bodies
  and the first WebSocket `response.create` frame.
- Define accepted type, invalid/empty handling, canonical affinity key, and
  fail-closed behavior before weighted fallback.

### R5-A6. Unknown/no-window/missing-reset human placeholders can recreate fake `0%`

Severity: important

Required revision:

- Define exact table/plain placeholders for unknown headroom, no data, and
  missing reset time.
- State that unknown or absent headroom must not render as `0% left`.

### R5-A7. JSON status schema is too small to audit the table/routing contract

Severity: important

Required revision:

- Include selected pool, next use, window slots, all relevant windows, reset
  metadata, salvage tie key, and enough safe fields to reconstruct the human
  explanation.
- Preserve the security contract: JSON may include local machine/debug account
  ids but no tokens, unsafe labels, prompts, bodies, auth headers, or secret
  material.

## Parent Verification

- `previous_response_id` is supported by existing repo evidence in
  `docs/specs/2026-06-20-codex-router-greenfield-spec.md`,
  `docs/specs/references/2026-06-20-research-evidence.md`, and the existing
  Plan 1B affinity rows.
- `WeightedDeficitSelector` uses input order as the tie breaker for equal
  accumulated weights, so the candidate order must be exact.
- The installed-Codex smoke harness exists, but the revised quota spec needed
  to say what additional reset-aware assertions make that harness satisfy this
  feature's e2e gate.

## Applied Same-Session Spec Revisions

The spec was revised after this ledger to add:

- `fallback` next-use semantics for selected unknown pools
- exact salvage tie key
- unknown/no-data human placeholders
- expanded JSON audit shape
- exact `previous_response_id` affinity extraction
- WebSocket preselection failure matrix
- explicit installed-Codex-through-router e2e acceptance

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until the revised spec
passes another spec-review gate.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r5/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R5 found accepted blockers in fallback UX, deterministic selector ordering, affinity extraction, and e2e proof acceptance.
