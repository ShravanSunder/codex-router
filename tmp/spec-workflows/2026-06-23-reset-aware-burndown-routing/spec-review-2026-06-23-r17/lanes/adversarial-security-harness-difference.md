# R17 Adversarial, Security, Harness, And Difference Lane

Verdict: needs revision

Coverage: read full 1990-line spec, R16/R17 ledgers, goal details, and current
routes, WebSocket, account-selection, state, headers/profile, and
installed-Codex anchors.

Accepted findings:

- Blocker: route/API e2e remains responses-only while router supports more
  APIs.
- Blocker: WebSocket direct-payload predicate is weaker than current fail-closed
  behavior.
- Important: top-level JSON auth-smuggling denylist is not tied to a supported
  route inventory.

Reducer route: spec-creation-swarm.
