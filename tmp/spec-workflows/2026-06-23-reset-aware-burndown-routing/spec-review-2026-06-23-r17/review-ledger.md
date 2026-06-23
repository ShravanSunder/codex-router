# R17 Reset-Aware Burn-Down Routing Spec Review Ledger

Date: 2026-06-23
Phase: spec-review-swarm
Reviewed commit: `7fd12b1`
Verdict: needs revision

## Coverage

Parent coverage:

- `reset-aware-burndown-routing-spec.md`: 1990 lines, read in chunks
  1-500, 501-1000, 1001-1500, and 1501-1990.
- R16 review ledger, R17 revision ledger, goal details, route classifier,
  upstream routing, account selection, burn-down, state, WebSocket, HTTP/SSE,
  and installed-Codex test-support anchors were inspected.

Lanes:

| Lane | Agent | Verdict |
| --- | --- | --- |
| whole-spec-coverage | Russell | needs revision |
| requirements-testability + validation-and-testability | Dirac | needs revision |
| contract-and-scope + architecture-boundaries + planning-readiness | Lagrange | needs revision |
| adversarial-crux + security-threat-model + harness-fit + spec-difference | Franklin | needs revision |

## Parent Reducer Verdict

The spec is not ready for `plan-creation-swarm`. R17 closed the R16 issues, but
review found new route/API coverage and runtime selection side-effect gaps that
would still let a plan under-prove the product path.

## Accepted Blockers

1. The proof matrix is too `/v1/responses`-centric. Current router support
   includes `POST /v1/responses`, WebSocket `/v1/responses`, `GET /v1/models`,
   `POST /v1/memories/trace_summarize`, `POST /v1/responses/compact`, and
   unsupported path rejection. The spec must require proof for every routed API
   or explicit fail-closed behavior.

2. Unsupported semantics are mixed. Raw proxy method/path rejection should use
   `unsupported_path`; assessment/status policy misses should use
   `unsupported_route_band`.

3. Runtime side effects are still under-specified for cooldown reuse,
   previous-response affinity hits, weighted fallback, WebSocket connection
   pins, route-band holds, and durable owner writes.

4. WebSocket direct-payload validation is weaker in prose than current safe
   behavior. The spec must require non-empty string `model`, top-level array
   `input`, and literal `stream=true` before selection.

## Accepted Important Findings

1. Refresh-status read surface needs exact return shape and missing-status
   semantics.
2. `RuntimeSelectedAccountDecision` should not duplicate envelope-owned
   selected-pool state.
3. Runtime weighted-deficit and hold state must be route-band partitioned.
4. Cooldown reuse must advance weighted-deficit state, unlike affinity hits.
5. Installed-Codex bearer proof needs transport-specific safe receipt fields.
6. HTTP/SSE top-level body denylist must be scoped to named routed JSON POST
   surfaces and covered by compatibility proof.

## Next Workflow

Return to `shravan-dev-workflow:spec-creation-swarm`. Do not start
`plan-creation-swarm` until these findings are revised and another
`spec-review-swarm` returns a parent-verified `ready` verdict.
