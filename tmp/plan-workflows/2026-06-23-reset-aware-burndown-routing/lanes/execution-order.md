# Execution Order Lane

Status: answered
Candidate evidence label: `execution-order-candidate-readonly-2026-06-23`
Security context: applicable

## Execution DAG Candidate

```text
gate 0: repo/source freshness and target-file cleanliness
  |
  +-- lane A: shared lower-level contracts
  |     core RouteBand, SafeAccountLabel, affinity typed IDs/HMAC
  |
  +-- lane B: pure burn-down assessment
  |     depends on lane A
  |
  +-- lane C: state and secret substrate
        depends on lane A

integration gate 1: lower-layer contract gate
  |
  +-- lane D: proxy HTTP/SSE runtime adapter and route inventory
  |
  +-- lane E: CLI quota status and refresh worker integration
  |
  +-- lane F: e2e/test-support harness preparation

integration gate 2: adapter contract gate
  |
  +-- lane G: WebSocket preselection/security path

security integration gate
  |
route-native black-box gate
  |
installed-Codex HTTP/SSE + WebSocket e2e gate
  |
final validation + implementation-review-swarm
```

## Accepted Checkpoint Boundaries

1. Core routing, redaction, and affinity primitives.
2. Pure burn-down assessment contract.
3. SQLite refresh overlay and affinity owner substrate.
4. HTTP/SSE runtime selection and route inventory.
5. Quota status and refresh worker integration.
6. WebSocket preselection and security call order.
7. Route-native and installed-Codex harness proof.
8. Final validation fixes after `implementation-review-swarm`.

## Accepted Sequencing Risks

- `quota.rs` is a conflict hotspot; status and refresh-worker work should be
  one lane or serialized.
- WebSocket work must delete/replace transcript convenience fields forbidden by
  the spec before e2e proof is accepted.
- Previous-response affinity cannot reuse the existing raw `AffinityKey` API;
  core/secret/state cutover must land first.
- The plan must go to `plan-review-swarm` before implementation execution.

## Receipt

Answered by execution-order lane. Parent accepted the DAG with one
clarification: state refresh and affinity persistence can be implemented as
serial substeps inside the same state checkpoint because both touch schema and
repository APIs.
