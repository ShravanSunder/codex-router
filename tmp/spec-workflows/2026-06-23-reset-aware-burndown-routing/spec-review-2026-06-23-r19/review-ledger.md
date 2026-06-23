# R19 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `455908d`
Verdict: needs revision

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1963 lines, read in chunks
  1-500, 501-1000, 1001-1500, and 1501-1963.
- R18 review ledger, R19 revision ledger, route classifier, HTTP/SSE service,
  WebSocket server/tunnel, account selection, burn-down, weighted selector, and
  installed-Codex harness anchors were inspected.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| blocker-closure | Hypatia | needs revision |
| selector-api-contract | Harvey | needs revision |
| websocket-harness | Pascal | ready |

## Parent Reducer Verdict

R19 is not ready for `plan-creation-swarm`. WebSocket/harness proof is ready,
and R19 closed the route inventory, wrong-method proof, unsupported JSON
surface, and most DTO-shape gaps. Two selector-contract issues remained:
legacy public-surface references and affinity-present call order.

## Accepted Blockers

1. The flat result-envelope cutover still had stale references to
   `BurnDownRouteBandAssessment.weighted_candidates` and
   `BurnDownRouteBandAssessment.accounts`.
2. HTTP/SSE and WebSocket routing-order sections made shared burn-down
   assessment look conditional on no affinity, contradicting the selected-pool
   before affinity rule and current selector flow.

## What Held

- `unsupported_path` and `unsupported_route_band` are separated.
- Wrong HTTP methods on supported paths have black-box fail-closed proof.
- Unsupported-route-band JSON is internal/test-only.
- WebSocket invalid auth and unsupported path proof requires non-101 local
  rejection, not post-upgrade close.
- First-frame, installed-Codex, and redaction proof obligations are intact.
- Non-capable route `previous_response_id` pass-through is explicit.

## Next Workflow

Return to `shravan-dev-workflow:spec-creation-swarm` for the small R20 cleanup,
then rerun focused spec review. Do not start `plan-creation-swarm` until review
returns a parent-verified `ready` verdict.
