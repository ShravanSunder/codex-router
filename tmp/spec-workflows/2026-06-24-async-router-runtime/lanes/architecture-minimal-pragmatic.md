# Lane: architecture-minimal-pragmatic

Status: completed
Agent: Singer

## Candidate Evidence

- claim: current runtime accepts concurrent connections by spawning one OS
  thread per accepted connection.
  source anchor: `crates/codex-router-proxy/src/server.rs:369`
  evidence class: direct observation
  decision bucket: supports
  design implication: accept-loop concurrency is not the full remaining problem.
  boundary impact: preserves existing boundary
  proof modality: test
  confidence: high

- claim: WebSocket tunnel remains blocking through handshake, upstream connect,
  and steady-state pump.
  source anchor: `crates/codex-router-proxy/src/websocket.rs:628`
  evidence class: direct observation
  decision bucket: supports
  design implication: async replacement must include the WebSocket tunnel/pump.
  boundary impact: changes boundary
  proof modality: test
  confidence: high

- claim: upstream HTTP/SSE transport remains blocking.
  source anchor: `crates/codex-router-proxy/src/upstream.rs:162`
  evidence class: direct observation
  decision bucket: supports
  design implication: runtime slice must include HTTP/SSE upstream transport,
  not only WebSocket.
  boundary impact: changes boundary
  proof modality: test
  confidence: high

- claim: policy semantics are already factored above the blocking transport.
  source anchor: `crates/codex-router-proxy/src/http_sse.rs:563`,
  `crates/codex-router-proxy/src/websocket.rs:320`
  evidence class: direct observation
  decision bucket: supports
  design implication: smallest safe slice is "async proxy runtime shell, same
  proxy policy."
  boundary impact: preserves existing boundary
  proof modality: schema
  confidence: high

## Parent Reduction

Accepted into the primary spec:

- no session picker in this slice
- change runtime/listener/HTTP transport/WebSocket transport first
- preserve auth/selection/affinity policy
- SQLx scope starts with runtime state access required by `serve`
- proof must include mixed WebSocket + HTTP/SSE concurrency
