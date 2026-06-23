# R20 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Input review: `spec-review-2026-06-23-r19/review-ledger.md`

## Applied Revisions

- Replaced remaining public `BurnDownRouteBandAssessment.*` references with
  `BurnDownRouteBandAssessmentResult` envelope fields.
- Made HTTP/SSE routing order build the shared
  `BurnDownRouteBandAssessmentResult` before route-scoped affinity enforcement.
- Made WebSocket routing order run reset-aware `responses` assessment before
  route-scoped affinity enforcement.
- Updated HTTP/SSE call-order proof wording so assessment precedes optional
  affinity.

## Verification

- Primary spec line count after revision: under the 2000-line artifact cap.
- Stale `BurnDownRouteBandAssessment.*` reference scan must return zero active
  hits before review.

## Next Workflow

Run focused `shravan-dev-workflow:spec-review-swarm` against R20. Do not
proceed to `plan-creation-swarm` until the parent reducer records verdict
`ready`.
