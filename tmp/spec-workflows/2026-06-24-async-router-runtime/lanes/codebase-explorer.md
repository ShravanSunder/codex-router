# Lane: codebase-explorer

Status: completed
Agent: Godel

## Candidate Evidence

- claim: fail-closed behavior before upstream is an existing invariant for local
  auth and route rejection, including WebSocket pre-accept rejection.
  source anchor: `crates/codex-router-proxy/src/server.rs`,
  `crates/codex-router-proxy/src/local_auth.rs`,
  `crates/codex-router-proxy/src/lib.rs`
  evidence class: direct observation
  decision bucket: supports
  design implication: async runtime cannot upgrade/connect upstream before
  preserving the rejection gates.
  boundary impact: preserves existing boundary
  proof modality: test
  confidence: high

- claim: route classification is a small explicit supported surface, not a
  generic proxy.
  source anchor: `crates/codex-router-proxy/src/routes.rs:16`
  evidence class: direct observation
  decision bucket: supports
  design implication: Hyper service must keep fail-closed route classification.
  boundary impact: preserves existing boundary
  proof modality: test
  confidence: high

- claim: current thread-per-connection hotfix does not make the tunnel a true
  async proxy.
  source anchor: `crates/codex-router-proxy/src/server.rs:369`,
  `crates/codex-router-proxy/src/websocket.rs:628`,
  `crates/codex-router-proxy/src/websocket.rs:723`
  evidence class: direct observation
  decision bucket: supports
  design implication: spec must replace blocking tunnel and prove bidirectional
  progress, not only accept-loop concurrency.
  boundary impact: changes boundary
  proof modality: test
  confidence: high

- claim: token rotation revokes active WebSocket sessions by generation today.
  source anchor: `crates/codex-router-proxy/src/websocket.rs:479`
  evidence class: direct observation
  decision bucket: supports
  design implication: async runtime needs a generation-scoped session registry
  and cancellation path.
  boundary impact: changes boundary
  proof modality: test
  confidence: high

## Parent Reduction

Accepted into the primary spec:

- preserve fail-closed route/auth semantics
- preserve first-frame WebSocket routing
- replace blocking per-connection transport, not only the accept loop
- keep token-generation revocation in runtime
- expand proof beyond current concurrent-accept tests
