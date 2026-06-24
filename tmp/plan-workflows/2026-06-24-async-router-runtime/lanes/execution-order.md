# Lane: execution-order

Status: answered
Reasoning effort: high
Security context: applicable

## Execution DAG Candidate

```text
gate 0: accepted plan artifact and matrix
  |
gate 1: shared ownership substrate
  |
gate 2: SQLx async state/auth boundary
  |
  +-- gate 3A: async HTTP/SSE Hyper path
  |
  +-- gate 3B: async WebSocket tokio-tungstenite path
  |
  +-- gate 3C: session registry / revocation / observability
  |
  +-- gate 3D: early structural guardrails
  |
integration gate 4: one merged async serve path
  |
gate 5: issue-closure regression proof
  |
gate 6: installed-Codex real-serve smoke/e2e harness
  |
gate 7: long-running soak and final proof pack
  |
implementation-review-swarm
```

## Ordering Rules

- Gate 1 is serial because `server.rs` is the shared entrypoint and the spec
  forbids dual production `serve` paths.
- Gate 2 is serial because HTTP and WebSocket both depend on request-time
  selection, credential resolution, state, and auth commit semantics.
- HTTP/SSE, WebSocket, registry/observability, and guardrail scaffolding may
  parallelize only after Gate 2 settles shared contracts.
- Final proof harnesses are serial on the real release `serve` path.

## Commit Checkpoints

- CP1: runtime ownership substrate and single-serve-path skeleton.
- CP2: SQLx async state/auth boundary and resolver contract cutover.
- CP3: early structural/dependency guardrail scaffolding.
- CP4: merged single async `serve` path through CLI/runtime entry.
- CP5: issue-closure proof rows green.
- CP6: final guardrails tightened to actual release `serve` reachability.
- CP7: installed-Codex smoke/e2e harness green.
- CP8: soak artifact and final proof pack.

## Replan Triggers

- Replan if Gate 1 cannot define one release async `serve` path without
  preserving a selectable blocking production path.
- Split before transport work if 3A/3B require simultaneous large edits in
  `server.rs`.
- Replan before implementation if any Issue Closure or Permanent Regression
  Guardrail row cannot attach to a slice-local checkpoint.
- Stop before e2e readiness if the installed-Codex harness cannot emit the
  required redacted continuity artifact.

## Completion Receipt

Returned execution DAG, safe/unsafe parallelism, validation gates, checkpoint
commits, and split/replan triggers.

Confidence: medium-high
