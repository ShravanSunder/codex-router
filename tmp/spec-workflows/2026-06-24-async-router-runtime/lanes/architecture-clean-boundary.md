# Lane: architecture-clean-boundary

Status: completed
Agent: Carver

## Candidate Evidence

- claim: `codex-router-proxy` should own async runtime orchestration, but not
  SQLx queries or schema evolution.
  source anchor: `crates/codex-router-proxy/src/server.rs`,
  `crates/codex-router-state/src/sqlite.rs`
  evidence class: direct observation
  decision bucket: supports
  design implication: move listener/task/upstream I/O into proxy while keeping
  persistence ownership in state.
  boundary impact: changes boundary
  proof modality: schema
  confidence: high

- claim: WebSocket semantics depend on a connection-scoped first-frame routing
  decision before upstream open.
  source anchor: `crates/codex-router-proxy/src/websocket.rs:320`,
  `crates/codex-router-proxy/src/websocket.rs:628`
  evidence class: direct observation
  decision bucket: supports
  design implication: async WebSocket service must keep a two-phase contract.
  boundary impact: preserves existing boundary
  proof modality: test
  confidence: high

- claim: previous-response affinity is already an explicit injected concern and
  should remain separate from transport loops.
  source anchor: `crates/codex-router-proxy/src/http_sse.rs`,
  `crates/codex-router-proxy/src/websocket.rs`
  evidence class: direct observation
  decision bucket: supports
  design implication: frame/body pumps should call recorder contracts rather
  than raw DB operations.
  boundary impact: preserves existing boundary
  proof modality: data/db/state
  confidence: high

## Ownership Sketch

```text
proxy runtime
  owns: Tokio, Hyper, WS sessions, cancellation, revocation
  uses: auth/state/secret-store contracts

auth
  owns: provider credential resolution and refresh semantics
  uses: state/secret-store contracts

state
  owns: SQLx pool, migrations, schema, account/quota/affinity persistence
```

## Parent Reduction

Accepted into the primary spec:

- SQLx belongs to state
- proxy owns runtime/session lifecycle
- auth does not depend on Hyper or WebSocket crates
- frame/body loops do not own raw persistence
