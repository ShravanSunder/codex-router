# Contract + Architecture + Spec Difference Lane

Status: answered
Verdict: needs revision
Coverage: full 837-line spec reviewed with current selector, affinity,
WebSocket, state, and CLI code anchors.

## Findings

- Blocker: `selected_next` belongs to pure assessment in the spec, but the live
  next pick is owned by proxy mutable selector state. Planning would have to
  invent a boundary.
- Blocker: previous-response affinity is only mentioned in WebSocket order and
  lacks owner, persistence, HTTP/SSE scope, and fail-closed behavior.
- Important: current WebSocket code-order delta is not named in current-state
  evidence; selection/resolution before first-frame parsing must be called out
  as a deliberate target change.

Completion receipt: answered, with anchors in target spec, greenfield spec,
plan, state `AffinityRepository`, selection affinity helper, and WebSocket code.
