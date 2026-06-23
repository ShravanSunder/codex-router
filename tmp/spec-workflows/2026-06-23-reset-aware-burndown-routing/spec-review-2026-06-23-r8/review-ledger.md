# Reset-Aware Burn-Down Routing Spec Review Ledger R8

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `c8c02e1886d06c344aa55d35288cf844daacb23b`
Review worktree: `/tmp/codex-router-r8-review.RBmbZ8`
Coverage: 1245 lines before R8 fixes, read by parent in chunks 1-250,
251-500, 501-750, 751-1000, 1001-1245
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Epicurus | answered | ready |
| requirements-testability + validation-and-testability + planning-readiness | Banach | answered | ready |
| contract-and-scope + architecture-boundaries + spec-difference | Lovelace | answered | needs revision |
| security-threat-model + WebSocket/protocol + affinity redaction | Copernicus | answered | needs revision |
| adversarial-crux + guardrail-codification + UX/status | Mencius | answered | ready |

## What Held

- R7 fixes for all-supplied account assessment, JSON envelope, local JSON vs
  shared-artifact redaction, and status structural guardrails are materially
  present.
- WebSocket first-frame preselection remains path-owned and allowlisted.
- The remaining issues are narrow affinity contract refinements.

## Accepted Findings

### R8-A1. Affinity key hash contract is underspecified

Severity: important

Required revision:

- Define exact algorithm, keyedness, encoding, output length, and construction
  owner for `affinity_key_hash`.
- Forbid raw canonical affinity-key persistence.
- Define hard schema cutover from existing raw affinity rows.
- Define duplicate/collision ambiguity as fail-closed.

### R8-A2. Previous-response owner route eligibility is ambiguous

Severity: important

Required revision:

- Map owner validity to burn-down availability classes.
- Choose whether `unknown` owners are valid or fail closed.
- Add proof rows for the selected behavior.

## Applied Same-Session Spec Revisions

The spec was revised after this ledger to add:

- full-length lowercase-hex HMAC-SHA-256 `affinity_key_hash`
- router-owned `router_affinity_hash_secret`
- one shared helper before storage/logging/tracing/audit
- hard schema cutover with no raw-key fallback
- duplicate or ambiguous owner rows fail closed
- owner route eligibility is `usable` or `reserve` only; `unknown`, `blocked`,
  and `excluded` fail closed
- affinity hash and owner-eligibility proof rows

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until the revised spec
passes another spec-review gate.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r8/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R8 found accepted affinity hash and owner-route-eligibility gaps, so planning remains blocked until the revised spec passes another review.
