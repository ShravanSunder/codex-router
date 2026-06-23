# Reset-Aware Burn-Down Routing Spec Review Ledger R6

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `8dab4631a8f2cdabfaaedb0be233f633f15fa04d`
Review worktree: `/tmp/codex-router-r6-review.8EyxlP`
Coverage: 1106 lines, read by parent in chunks 1-200, 201-400, 401-600, 601-800, 801-1000, 1001-1106
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Linnaeus | answered | needs revision |
| requirements-testability + validation-and-testability + planning-readiness | Pauli | answered | needs revision |
| contract-and-scope + architecture-boundaries + spec-difference | Jason | answered | needs revision |
| security-threat-model + WebSocket/protocol | Poincare | answered | ready |
| adversarial-crux + guardrail-codification + UX/status | Noether | answered | ready |

## What Held

- R5 fixes for all-unknown fallback, salvage tie key, WebSocket failure matrix,
  `previous_response_id` extraction, JSON audit shape, and local e2e acceptance
  are materially present.
- Security/WebSocket and UX/status lanes reported ready.
- Remaining findings are narrow spec-clarity issues, not design reversals.

## Accepted Findings

### R6-A1. WebSocket first-frame local field allowlist is not exact

Severity: important

Required revision:

- State that `/v1/responses` path alone fixes the route band.
- Enumerate the exact preselection first-frame fields: top-level `type` and
  top-level `previous_response_id` only.
- Require canary proof that non-allowlisted fields such as `model`, `input`,
  `metadata`, `tools`, and prompt text are not read or logged before selection.

### R6-A2. Account eligibility ownership is overloaded

Severity: important

Required revision:

- Replace proxy-owned `account eligibility` with proxy-owned fact adaptation,
  affinity enforcement, fairness state, and runtime exact selection.
- State that burn-down owns pure exclusion/classification from supplied facts.
- Keep credential resolution and secret-store reads outside burn-down.

### R6-A3. Unknown fallback final reason conflicts with raw evidence reason

Severity: blocker

Required revision:

- Split raw quota evidence into `quota_evidence_reason`.
- Assign final public/audit `routing_reason` only after route-band pool mapping.
- Preserve all-unknown fallback public reasons without losing missing-reset,
  missing-window, unknown-window, or no-window evidence.

### R6-A4. Partial v1 5h/weekly window sets are not normatively collapsed

Severity: important

Required revision:

- Define the expected v1 response quota shape as one 5h window and one weekly
  window.
- If exactly one expected window is missing, the account is `unknown`, never
  normal usable, and the missing display slot renders `no data`.

## Applied Same-Session Spec Revisions

The spec was revised after this ledger to add:

- exact WebSocket first-frame preselection allowlist
- proxy/selection eligibility ownership split
- `availability=excluded`, `routing_exclusion`, and status mappings for
  disabled or missing-credential accounts
- `quota_evidence_reason` as raw evidence separate from final `routing_reason`
- `missing_expected_window` collapse, public reason, JSON field, and proof rows

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until the revised spec
passes another spec-review gate.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r6/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R6 found accepted clarity gaps in first-frame field allowlisting, eligibility ownership, unknown final reasons, and partial-window collapse.
