# Plan Review Lane: Whole Plan Cohesion

Verdict: `needs revision`

## Accepted Findings

### Blocker: previous-response owner-record writes were unowned

- Problem: the plan covered hash storage and owner lookup, but no task wrote
  owner rows from successful upstream response IDs.
- Failure: a first response could succeed, then the continuation would have no
  owner record and would route incorrectly or fail unexpectedly.
- Required edit: HTTP/SSE writes from allowlisted upstream `id`; WebSocket
  writes from allowlisted top-level `response.id`; no raw ID leakage.
- Folded into plan: T3, T6, RP-09.

### Important: T5/T6 were unsafe to parallelize

- Problem: both slices touched the same local-auth primitive and proxy surfaces.
- Failure: HTTP and WebSocket auth behavior could diverge.
- Required edit: serialize T3 -> T5 -> T6 and make T6 consume T5's shared
  local-auth contract.
- Folded into plan: T5, T6, Execution DAG, Parallel Work Rules.

### Important: final validation dropped supply-chain gates

- Problem: T11 omitted `cargo deny check` and `cargo audit`.
- Required edit: add them to final validation.
- Folded into plan: T11.

