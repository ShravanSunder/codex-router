# R18 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `5635c6e`
Verdict: needs revision

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1992 lines, read in chunks
  1-500, 501-1000, 1001-1500, and 1501-1992.
- R17 review ledger, R18 revision ledger, goal details, route classifier,
  HTTP/SSE routing, WebSocket routing, account selection, burn-down, weighted
  selector, state repositories, SQLite projections, and installed-Codex harness
  anchors were inspected.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-spec-coverage | Maxwell | needs revision |
| requirements-testability + validation-and-testability | Nash | needs revision |
| contract-and-scope + architecture-boundaries + planning-readiness | Laplace | needs revision |
| adversarial-crux + security-threat-model + harness-fit + spec-difference | Halley | needs revision |

## Parent Reducer Verdict

R18 is not ready for `plan-creation-swarm`. The all-routed-API direction and
account-selection model are materially stronger, but review found remaining
contract contradictions that would let planning under-specify method rejection,
HTTP route ordering, previous-response affinity scoping, the shared assessment
DTO shape, and WebSocket pre-upgrade proof.

## Accepted Blockers

1. HTTP/SSE routing order still collapsed raw classifier misses into
   `unsupported_route_band` and scoped the normative order too narrowly to
   response/affinity routes.
2. Unsupported HTTP wrong-method cases on otherwise supported paths were covered
   by the route inventory but not by the black-box fail-closed proof gate.
3. The shared assessment/result contract was still two-shaped: variant-style
   constructors and a flat top-level envelope both appeared normative.
4. WebSocket invalid local auth and unsupported-path proof did not require an
   observable handshake/connect failure or non-101 local rejection, so
   post-upgrade close could pass a weaker proof.

## Accepted Important Findings

1. Routes marked not previous-response capable need explicit behavior for
   top-level `previous_response_id`; v1 should not treat it as router affinity
   metadata outside `/v1/responses`.
2. `BurnDownAccountInput` should not carry a per-account `route_band` in
   addition to the top-level assessment route band.
3. Unsupported-route-band JSON should be internal/test-only in v1, not an
   implied new user-facing debug/status command.

## Rejected Or Deferred

- No accepted finding weakens the R18 account-selection direction. The intended
  implementation delta remains: cooldown reuse debits weighted fairness,
  previous-response affinity does not, route-band holds/fairness are partitioned,
  and reserve-owner affinity continuation may be outside the selected weighted
  pool when still route-eligible.

## Next Workflow

Return to `shravan-dev-workflow:spec-creation-swarm`. Do not start
`plan-creation-swarm` until these findings are revised and another
`spec-review-swarm` returns a parent-verified `ready` verdict.
