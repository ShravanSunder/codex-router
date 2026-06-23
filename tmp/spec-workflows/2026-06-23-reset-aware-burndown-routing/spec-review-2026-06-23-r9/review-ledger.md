# Reset-Aware Burn-Down Routing Spec Review Ledger R9

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `5e39282dea9defdfabff60af07593e0605f5592e`
Review worktree: `/tmp/codex-router-r9-review.68OnKV`
Coverage: 1277 lines before R9 fixes, read by parent in chunks 1-260,
261-520, 521-780, 781-1040, 1041-1277
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + requirements-testability + validation-and-testability + planning-readiness | Einstein | answered | ready |
| contract-and-scope + architecture-boundaries + security-threat-model + WebSocket/protocol | Ampere | answered | needs revision |
| adversarial-crux + guardrail-codification + UX/status | Mendel | answered | ready |

## What Held

- R8 fixes for HMAC algorithm/domain/encoding, owner route eligibility,
  duplicate ambiguity fail-closed behavior, and no raw-key persistence are
  materially present.
- R7-ready UX/status/JSON/WebSocket surfaces remain intact.

## Accepted Findings

### R9-A1. Affinity hash secret lifecycle is underspecified

Severity: important

Required revision:

- Generate `router_affinity_hash_secret` once per router root.
- Persist it independently from local bearer tokens, OAuth/account credentials,
  and credential generations.
- Do not rotate it automatically or manually in v1.
- Local bearer-token rotation, OAuth token refresh, account credential rotation,
  server restart, and quota refresh must not change it.
- If missing, unreadable, or replaced, existing owner rows are ignored or purged
  and continuation requests fail closed before weighted fallback.
- Add proof rows for lifecycle behavior.

## Applied Same-Session Spec Revisions

The spec was revised after this ledger to add:

- generated-once-per-router-root affinity hash secret lifecycle
- no v1 hash-secret rotation path
- independence from bearer/account credential rotation and restarts
- fail-closed owner-row invalidation when the secret is missing, unreadable, or
  replaced
- lifecycle proof rows

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until the revised spec
passes another spec-review gate.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r9/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R9 found an accepted affinity hash-secret lifecycle gap, so planning remains blocked until the revised spec passes another review.
