# R17 Whole-Spec Coverage Lane

Verdict: needs revision

Coverage: read full 1990-line spec, R16 review ledger, R17 revision ledger,
goal details, and live route/upstream/selection/state/WebSocket/test-support
anchors.

Accepted findings:

- Blocker: every routed API is not converted into an explicit quota-routing or
  fail-closed proof obligation.
- Important: route-band partitioning of runtime weighted-deficit and hold state
  is implicit.
- Important: cooldown reuse must state whether it advances weighted-deficit
  state.

Reducer route: spec-creation-swarm.
