# R17 Contract, Boundaries, And Planning Lane

Verdict: needs revision

Coverage: read full 1990-line spec, R16/R17 ledgers, goal details, and current
route classifier, account selection, burn-down, state, HTTP/SSE, WebSocket, and
quota anchors.

Accepted findings:

- Blocker: missing route/API inventory and per-route acceptance gate.
- Blocker: cooldown, affinity, weighted fallback, and connection-pin state
  mutations are not exact enough.
- Important: refresh-status read surface is named but not contract-closed.
- Important: runtime decision DTO duplicates envelope-owned selected-pool state.

Reducer route: spec-creation-swarm.
