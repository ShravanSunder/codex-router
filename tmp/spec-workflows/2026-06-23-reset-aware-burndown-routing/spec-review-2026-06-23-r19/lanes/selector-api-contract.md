# R19 Selector And Routed-API Contract Lane

Verdict: needs revision

Accepted by parent:

- Blocker: HTTP/SSE and WebSocket order made shared assessment look conditional
  on no affinity. The contract must state assessment is built before affinity
  enforcement so affinity can validate owner route-eligibility against the same
  result envelope.

What held:

- Pure assessment owns route-band policy.
- Routed API inventory covers current HTTP/SSE, WebSocket, unsupported path, and
  unsupported method cases.
- Non-capable route `previous_response_id` pass-through is explicit and
  testable.

Receipt:

- Source anchors: spec lines 1-1963, `routes.rs`, `account_selection.rs`,
  `burn_down.rs`, `weighted_deficit.rs`, R18 review ledger, R19 revision ledger.
- Parent reducer wrote this lane summary from the subagent candidate output.
