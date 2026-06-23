# R16 Adversarial, Security, Harness, And Difference Lane

Verdict: needs revision

Coverage: reviewed current-state, requirements, boundary, refresh, auth,
WebSocket, proof, and workflow-gate sections plus account-selection, HTTP/SSE,
local-auth, state, and WebSocket code anchors.

Accepted findings:

- Blocker: refresh-overlay API contradiction can leak stale evaluation back
  into proxy or CLI.
- Blocker: HTTP/SSE body-token rejection is mandatory but does not define
  inspection scope.
- Important: current-state evidence understates that current affinity requires
  owner membership in `weighted_candidates`.

Contested tradeoff resolved by R17: HTTP/SSE body-token rejection mirrors the
WebSocket privacy model as a top-level JSON field-name check only.

Reducer route: spec-creation-swarm.
