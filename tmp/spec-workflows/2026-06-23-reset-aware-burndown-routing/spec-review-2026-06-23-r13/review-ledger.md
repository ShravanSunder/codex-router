# R13 Spec Review Ledger

Date: 2026-06-23
Status: needs revision; accepted findings folded into spec in this checkpoint

## Source

- Baseline commit: `5e5a1c4 docs: resolve r12 quota spec findings`
- Review worktree: `/tmp/codex-router-r13-review.jSdi0u`
- Target spec:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- Coverage before R13 fixes: 1665 lines. Parent read chunks 1-280, 281-560,
  561-840, 841-1120, 1121-1400, and 1401-1665.

## Lanes

| Lane | Agent | Verdict | Parent result |
| --- | --- | --- | --- |
| whole-spec-executable-contract | Gibbs (`019ef577-de5f-78f2-a6b6-8a3f3e832293`) | needs revision | accepted |
| websocket-auth-affinity-security | Euclid (`019ef578-0276-7780-9c2d-efe8589a6efb`) | needs revision | accepted |
| architecture-boundaries-plan-readiness | Herschel (`019ef578-275f-7532-a0dc-608a673ec846`) | needs revision | accepted |

## Accepted Findings

1. Local-auth rejection semantics still allowed a planner to accept mixed-carrier
   requests where `X-Codex-Router-Token` was present alongside a forbidden token
   carrier.
2. HTTP/SSE response-creating and previous-response-capable routes did not have
   an explicit affinity-secret preselection order matching the fail-closed
   invariant.
3. WebSocket first-frame step wording could be read as allowing additional body
   fields before selection, despite the allowlist contract.
4. Route-band policy ownership was split: `BurnDownRouteBandAssessmentInput`
   carried `route_band_policy` while selection also owned policy lookup and
   unsupported-band results.
5. The unsupported route-band branch lacked a normative payload, stable reason,
   JSON/status surface, and caller behavior.
6. Refresh persistence required stale/error metadata but did not define the
   durable `quota_refresh_status` shape or success/failure transition semantics.
7. `window_slots.source_window_ids` was normative output without any stable
   input/window identifier contract.

## Revision Applied

The spec now defines:

- selection-owned `assess_route_band(input) -> BurnDownRouteBandAssessmentResult`
  with no caller-supplied `route_band_policy`
- `UnsupportedRouteBandAssessment` payload and machine reason
  `unsupported_route_band`
- `quota_refresh_status` durable record and success/failure transition rules
- v1 `window_slots` without `source_window_ids`
- mixed-carrier local-auth failure semantics
- explicit HTTP/SSE routing order for response-creating and
  previous-response-capable routes
- WebSocket preselection wording that forbids parsing additional first-frame
  fields before selection
- proof rows for unsupported-band payload, refresh persistence, HTTP/SSE
  call-order, and mixed-carrier auth

## Parent Verdict

R13 did not pass the hard gate. Accepted findings were folded into the spec.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: R13 accepted findings have been folded into the spec; run another adversarial spec review before any plan creation.
