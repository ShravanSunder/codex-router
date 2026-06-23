# R12 Spec Review Ledger

Date: 2026-06-23
Status: needs revision; accepted findings folded into spec in this checkpoint

## Source

- Baseline commit: `195cb74 docs: resolve r11 quota spec findings`
- Review worktree: `/tmp/codex-router-r12-review.8otShY`
- Target spec:
  `tmp/spec-workflows/2026-06-23-reset-aware-burndown-routing/reset-aware-burndown-routing-spec.md`
- Coverage before R12 fixes: 1545 lines. Parent read chunks 1-320,
  321-640, 641-960, 961-1120, 1121-1280, 1281-1440, and 1441-1545.

## Lanes

| Lane | Agent | Verdict | Parent result |
| --- | --- | --- | --- |
| whole-spec-executable-contract | Leibniz (`019ef56b-ac5d-7aa1-ba28-d4520bcd2a96`) | needs revision | accepted |
| websocket-auth-affinity-security | Dewey (`019ef56b-cb41-7ed2-811a-e2f7c0279d83`) | needs revision | accepted |
| architecture-boundaries-plan-readiness | Galileo (`019ef56b-ea22-7343-976c-d55388ed3b86`) | needs revision | accepted |

## Accepted Findings

1. The local router auth contract was underspecified. The spec fixed generated
   Codex profile auth to `env_http_headers`, but did not explicitly reject
   Authorization bearer, `env_key`, query, cookie, subprotocol, or body-token
   fallback surfaces on router ingress.
2. WebSocket routing omitted the preselection affinity-hash-secret load/create
   step for no-affinity `response.create` requests, even though `/v1/responses`
   can create previous-response owner records.
3. Unknown-pool public reason mapping was contradictory: raw unknown evidence
   reasons had precedence before all-unknown fallback mappings, making
   `unknown_fallback_preferred` and `unknown_fallback_available` unreachable.
4. The shared assessment DTO did not expose all data required by the status
   contract, including `next_use`, `salvage_tie_key`, `window_slots`,
   `windows[]`, and safe presentation fields.
5. Route-band policy lookup was selection-owned, but unsupported route bands did
   not have a route-level `BurnDownRouteBandAssessmentResult` surface consumed
   by proxy, CLI, status, tests, and smoke proof.
6. Non-blocking quota refresh was specified mostly as proof, not as a normative
   refresh lifecycle with boot scheduling, periodic refresh, transient failure
   preservation, and ownership of persisted state transitions.
7. Goal details still pointed at older absolute main-checkout review artifacts
   and left key proof rows as "must be defined by plan-creation-swarm" even
   though the spec already defines concrete proof families.

## Revision Applied

The spec now defines:

- `BurnDownRouteBandAssessmentResult::ok(...)` and
  `::unsupported_route_band`, with proxy/CLI/status consuming the same
  route-level result
- selection-owned safe presentation fields for `next_use`, `routing_reason`,
  `window_slots`, `windows[]`, and `salvage_tie_key`
- selected-pool-aware unknown fallback reason precedence
- a normative quota refresh lifecycle and ownership contract
- local router auth ingress accepted and forbidden surfaces
- WebSocket affinity hash-secret load/create before selection
- WebSocket failure matrix and proof rows for affinity-secret-unavailable
- local-auth negative proof rows for HTTP/SSE and WebSocket
- goal-details required reading and proof rows aligned to the current spec and
  latest review artifact

## Parent Verdict

R12 did not pass the hard gate. Accepted findings were folded into the spec.

phase_result: needs_revision
recommended_next_workflow: shravan-dev-workflow:spec-review-swarm
recommended_transition_reason: R12 accepted findings have been folded into the spec; run another adversarial spec review before any plan creation.
