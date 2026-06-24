# Lane: risk-security

Status: timed out
Agent: Zeno

## Parent Fallback Risk Notes

The dedicated risk/security lane did not return before artifact synthesis.
The parent included security-sensitive requirements from the codebase scan and
existing product spec, then routed the draft to `spec-review-swarm` for a
second-pass adversarial review.

Risk inputs accepted into the draft:

- local router auth must reject before upstream egress
- local auth tokens and upstream credentials must not be logged or forwarded
- route classification remains fail-closed
- WebSocket first-frame parsing is bounded and only for routing metadata
- prompt/tool/message payloads are not policy inputs
- token-generation revocation remains required for active WebSocket sessions
- proof must include local close/upstream wait termination to catch the live
  stuck-session failure
