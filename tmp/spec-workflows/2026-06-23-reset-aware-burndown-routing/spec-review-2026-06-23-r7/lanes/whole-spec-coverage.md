# R7 Lane: Whole-Spec Coverage + Progressive Disclosure

Verdict: needs revision
Agent: Aquinas

## Candidate Findings

### Important: assessment construction contradicts excluded-row status contract

- Evidence: `reset-aware-burndown-routing-spec.md` previously required disabled
  and missing-credential accounts to return `availability=excluded`, but the
  normative pool-order step said to build assessments only for enabled accounts
  with active credentials.
- Failure path: planning could filter excluded accounts before burn-down,
  forcing CLI/status to reimplement eligibility and losing shared status rows.
- Refinement input: build assessments for every supplied account fact row;
  keep `excluded` and `blocked` in `accounts`, but never in
  `weighted_candidates`.

## What Held

- WebSocket first-frame allowlist, route-band path ownership,
  `quota_evidence_reason`, missing expected windows, and progressive-disclosure
  shape were otherwise coherent.

Completion receipt: answered with anchors.
Confidence: high
