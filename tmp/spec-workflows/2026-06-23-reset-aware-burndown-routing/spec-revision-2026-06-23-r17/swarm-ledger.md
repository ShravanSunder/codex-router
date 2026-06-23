# R17 Reset-Aware Burn-Down Routing Spec Revision Ledger

Date: 2026-06-23
Phase: spec-creation-swarm
Input review: `spec-review-2026-06-23-r16/review-ledger.md`

## Parent Synthesis

R17 folds the accepted R16 review findings back into the primary spec. The
revision keeps the design in spec space only; implementation planning remains
blocked until a new spec review returns `ready`.

## Applied Revisions

- Canonicalized the refresh read path as
  `SelectorQuotaRepository::selector_inputs_for_route_band(route_band, now_unix_seconds)`
  plus an explicit refresh-status read surface.
- Defined legacy selector rows with missing refresh metadata as stale on first
  post-upgrade read before successful refresh.
- Demoted raw unknown evidence reasons to `quota_evidence_reason` only. Public
  `routing_reason` now stays pool-based: `held_unknown`,
  `unknown_fallback_preferred`, or `unknown_fallback_available`.
- Expanded `RuntimeSelectedAccountDecision` so proxy/audit/test surfaces carry
  the shared `BurnDownRouteBandAssessmentResult` envelope instead of
  reconstructing route-level fields.
- Defined successful previous-response affinity side effects: no weighted
  deficit advancement, runtime hold refresh, and no mutation of pure
  `weighted_candidates`.
- Split accepted local auth from protocol-owned auth-smuggling checks. HTTP/SSE
  body and WebSocket first-frame checks are narrow top-level JSON field-name
  validators with no nested prompt/body scanning.
- Made installed-Codex generated-profile bearer proof mandatory with safe
  router-side observables.
- Kept the primary spec under the 2000-line artifact cap.

## Verification

- Primary spec line count after revision: 1990 lines.
- Targeted stale-phrase scan over the primary spec found no lingering one-arg
  selector API contract, stale marker ownership, optional bearer proof wording,
  public unknown `needs_refresh` enum, or ambiguous request-body-token wording.

## Next Workflow

Run `shravan-dev-workflow:spec-review-swarm` against R17. Do not proceed to
`plan-creation-swarm` until the parent reducer records verdict `ready`.
