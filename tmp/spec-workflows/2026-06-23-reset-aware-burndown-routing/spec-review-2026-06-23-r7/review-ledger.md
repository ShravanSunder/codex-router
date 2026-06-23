# Reset-Aware Burn-Down Routing Spec Review Ledger R7

Date: 2026-06-23
Reviewed artifact: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
Reviewed commit baseline: `5dd58c8259c30bdce0da84a28aa9704492379584`
Review worktree: `/tmp/codex-router-r7-review.8GCXy0`
Coverage: 1151 lines before R7 fixes, read by parent in chunks 1-230,
231-460, 461-690, 691-920, 921-1151
Verdict: needs revision

## Lanes Run

| Lane | Agent | Status | Verdict |
| --- | --- | --- | --- |
| whole-spec-coverage + progressive-disclosure | Aquinas | answered | needs revision |
| requirements-testability + validation-and-testability + planning-readiness | Kepler | answered | needs revision |
| contract-and-scope + architecture-boundaries + spec-difference | Hilbert | answered | needs revision |
| security-threat-model + WebSocket/protocol | Nash | answered | ready |
| adversarial-crux + guardrail-codification + UX/status | Plato | answered | needs revision |

## What Held

- R6 fixes for WebSocket path-owned route band, exact first-frame read
  allowlist, `quota_evidence_reason`, final `routing_reason`, and partial
  v1 missing-window collapse are materially present.
- Security/WebSocket lane reported ready.
- Local e2e, non-blocking startup/request/status, redaction, and WebSocket
  proof expectations are strong enough to plan after the accepted findings below
  are fixed.

## Accepted Findings

### R7-A1. Assessment inclusion contradicts excluded-account status

Severity: blocker

Required revision:

- Build assessments for every supplied route-band account fact row, including
  disabled accounts and accounts without active credentials.
- Keep `excluded` and `blocked` assessments in `accounts` for status, JSON,
  logs, and proof.
- Ensure `excluded` and `blocked` assessments never enter `weighted_candidates`.
- Add proof rows for disabled and missing-active-credential accounts returning
  `availability=excluded`, mapping to stable reasons, rendering safely, and
  avoiding unsafe labels or secret material.

### R7-A2. Previous-response affinity lacks owner-record creation contract

Severity: blocker

Required revision:

- Define `PreviousResponseOwnerRecord`.
- Define HTTP/SSE and WebSocket owner-record write triggers.
- Store selected account id, active credential generation, route band, source
  transport, creation time, and hashed affinity key.
- Define allowed upstream response id fields and redaction limits.
- Add proof for owner-record creation and no raw previous-response id leakage.

### R7-A3. JSON status contract is not normative enough

Severity: important

Required revision:

- Add a compact JSON envelope showing top-level route fields,
  `preferred_next_account_id`, `weighted_candidates[]`, and `accounts[]`.
- Put per-account status fields, `window_slots.{5h,weekly}`, and `windows[]`
  under `accounts[]`.
- Distinguish route-level `preferred_next_account_id` from
  `accounts[].preferred_next`.

### R7-A4. Raw local JSON account ids conflict with shared artifact redaction

Severity: important

Required revision:

- State that raw local `--format json` stdout may include `account_id`.
- State that any persisted/shared artifact containing JSON output must redact or
  hash `account_id`.
- Add proof rows for both raw local JSON utility and shared-artifact redaction.

### R7-A5. Status proof lacks structural guardrails

Severity: important

Required revision:

- Add guardrails that default status is account-centric for the user quota
  route.
- Assert one logical row per account, with at most one blank-account
  continuation line.
- Assert no unrelated route-band rows or labels appear in default status.

## Applied Same-Session Spec Revisions

The spec was revised after this ledger to add:

- all-supplied-account assessment before selected-pool filtering
- explicit excluded/blocked candidate exclusion and proof rows
- previous-response owner record shape, creation triggers, credential generation
  staleness semantics, allowlisted upstream response id parsing, and pin-write
  proof
- local JSON stdout versus shared artifact redaction split
- normative JSON envelope
- default status structural guardrails against duplicate logical rows and
  unrelated route-band noise

## Verdict

Needs revision. Do not route to `plan-creation-swarm` until the revised spec
passes another spec-review gate.

phase_result: needs_revision
evidence: `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/spec-review-2026-06-23-r7/review-ledger.md`
recommended_next_workflow: `shravan-dev-workflow:spec-creation-swarm`
recommended_transition_reason: R7 found accepted blockers in assessment inclusion and previous-response owner-record creation, plus accepted JSON/status guardrail gaps.
