# Plan Review Lane: Security And Reliability

Verdict: `needs revision`

## Accepted Findings

### Blocker: owner-record writes from allowlisted upstream IDs were unowned

- Problem: storage and lookup were planned, but runtime write paths were not.
- Failure: continuations could miss affinity after a successful first response.
- Required edit: add HTTP/SSE and WebSocket owner-record writes from allowlisted
  response fields only.
- Folded into plan: T3, T6, RP-09.

### Blocker: HTTP/SSE `affinity_secret_unavailable` was not planned

- Problem: WebSocket had fail-closed secret loading; HTTP/SSE did not.
- Failure: HTTP/SSE could select credentials or open upstream while the secret
  store is unavailable.
- Required edit: add HTTP/SSE secret gate before selector, credential, auth
  injection, and upstream open.
- Folded into plan: T3, RP-10.

### Important: WebSocket local-auth surface was incomplete

- Problem: subprotocol smuggling, mixed-carrier mismatch, and manual header
  success were not owned.
- Required edit: add shared local-auth matrix and WebSocket ingress matrix.
- Folded into plan: T5, T6, RP-06, RP-11.

### Important: secret-loss recovery was only a note

- Problem: missing/replaced affinity secret behavior was in rollback notes, not
  an executable task/proof.
- Required edit: move recovery behavior and tests into T2/T6.
- Folded into plan: T2b, T2c, T6.

