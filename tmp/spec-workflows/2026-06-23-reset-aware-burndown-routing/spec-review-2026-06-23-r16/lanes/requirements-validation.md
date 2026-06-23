# R16 Requirements And Validation Lane

Verdict: needs revision

Coverage: reviewed spec lines 1-1968, requirements/proof sections, and current
state, WebSocket, installed-Codex test-support, CLI, and burn-down anchors.

Accepted findings:

- Blocker: generated-profile bearer e2e proof is optional, so R8 can pass
  without proving installed Codex authenticated to the local router with
  `Authorization: Bearer`.
- Important: legacy selector rows without `quota_refresh_status` have no
  explicit first-read/bootstrap semantics.

Reducer route: spec-creation-swarm.
