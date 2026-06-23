# R16 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `4868306`
Verdict: needs revision

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1968 lines, read in chunks
  1-500, 501-1000, 1001-1500, and 1501-1968.
- R16 revision ledger, R15 review ledger, goal details, and current state,
  proxy, WebSocket, HTTP/SSE, test-support, and selection anchors were
  inspected.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-spec-coverage | Cicero | needs revision |
| requirements-testability + validation-and-testability | Carver | needs revision |
| contract-and-scope + architecture-boundaries + planning-readiness | Hilbert | needs revision |
| adversarial-crux + security-threat-model + harness-fit + spec-difference | Carson | needs revision |

## Parent Reducer Verdict

The spec is not ready for `plan-creation-swarm`. R16 fixed several R15 issues,
but still left planner-invented API, proof, security, and runtime-state choices.

## Accepted Blockers

1. Refresh-overlay read ownership is internally inconsistent. The spec exposed
   both `selector_inputs_for_route_band(route_band)` and
   `selector_inputs_for_route_band(route_band, now_unix_seconds)` without one
   canonical state API or DTO boundary.

2. WebSocket and HTTP/SSE local-auth/auth-smuggling ownership is still fuzzy.
   Header auth, mixed accepted carriers, HTTP body forbidden carriers, and
   WebSocket first-frame forbidden carriers need exact owners and detection
   scope.

3. Installed-Codex generated-profile bearer proof is optional. The e2e fixture
   must require a safe router-side observable proving local receipt of
   `Authorization: Bearer`.

4. Goal details still contain stale active instructions. Required reading,
   `phase_result: complete`, and rejected `env_http_headers` guidance are not
   clearly demoted from active source-of-truth state.

5. HTTP/SSE body-token rejection is mandatory but under-specified. The spec
   must define whether detection is top-level-only, JSON-only, nested scanning,
   or some other rule.

## Accepted Important Findings

1. Unknown quota public output conflicts between `held`, `fallback`, and
   `needs refresh`. Raw unknown evidence reasons should either be public
   reasons or evidence-only, but not both.

2. Legacy refresh bootstrap is missing. Preexisting selector rows with no
   `quota_refresh_status` need explicit first-read behavior.

3. The route-result envelope is defined for assessment/status, but the proxy
   runtime selection DTO does not yet carry it as the authoritative transport.

4. Previous-response affinity side effects on weighted-deficit state and
   connection/account holds are undefined.

5. Current-state evidence understates the selector delta: current affinity
   requires owner membership in `weighted_candidates`, while the target allows
   usable/reserve continuation owners outside the selected pool.

6. Review-to-plan transition needs an authoritative parent reducer receipt, not
   chat-only readiness.

## Next Revision Inputs

- Use one `codex-router-state` read API, including `now_unix_seconds`, and one
  refresh-status DTO/read surface.
- Split accepted-carrier local auth from protocol-owned top-level
  auth-smuggling validators; define HTTP body scope.
- Require `local_auth_carrier=authorization_bearer` plus
  `local_auth_validated=true` or an equivalent safe router-side observable in
  installed-Codex e2e proof.
- Demote unknown raw evidence reasons to `quota_evidence_reason` if public
  routing reasons are pool-based.
- Define legacy selector-row bootstrap behavior before successful refresh.
- Carry the shared assessment envelope in the proxy runtime decision DTO.
- Define affinity hit side effects on weighted-deficit and route-band holds.
- Update goal details so active guidance points at the latest artifacts and
  parent-verified review verdict `ready`.

## Next Workflow

Return to `shravan-dev-workflow:spec-creation-swarm`. Do not start
`plan-creation-swarm` until these findings are revised and another
`spec-review-swarm` returns a parent-verified `ready` verdict.
