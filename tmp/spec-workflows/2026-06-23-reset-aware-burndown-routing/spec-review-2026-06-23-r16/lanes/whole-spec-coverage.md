# R16 Whole-Spec Coverage Lane

Verdict: needs revision

Coverage: reviewed spec lines 1-1968 plus R16 revision ledger, R15 review
ledger, goal details, and current server/WebSocket/account-selection/state
anchors.

Accepted findings:

- Blocker: refresh overlay exposes both one-argument and two-argument selector
  read APIs.
- Blocker: goal details retain stale active required-reading, auth, and
  `phase_result: complete` guidance.
- Important: unknown quota public output conflicts between held, fallback, and
  needs-refresh mappings.

Reducer route: spec-creation-swarm.
