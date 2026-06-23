# Plan Review Lane: Testability And Validation

Verdict: `needs revision`

## Accepted Findings

### Blocker: WebSocket auth compatibility proof was incomplete

- Problem: proof matrix missed WebSocket manual header success and subprotocol
  token rejection.
- Required edit: add cross-transport auth matrix and WebSocket ingress matrix.
- Folded into plan: T5, T6, RP-06, RP-11.

### Blocker: installed-Codex HTTP and WebSocket proof was not isolated

- Problem: the existing smoke path combined transports, while the plan claimed
  separate T9 and T10 proof.
- Failure: one transport could regress while the combined proof remained
  ambiguous.
- Required edit: create exact transport-specific ignored test prefixes and smoke
  wrapper invocations.
- Folded into plan: T8a, T9, T10, T11, RP-14.

### Blocker: redaction gate was too narrow

- Problem: final canary scans did not include review/receipt artifacts, raw
  previous-response IDs, or shared JSON `account_id` leakage.
- Required edit: expand artifact and canary list.
- Folded into plan: T11, RP-15.

### Important: route-native command was not owned by the harness task

- Problem: T11 referenced `route_native_`, but T8 did not require creating that
  test prefix.
- Required edit: split T8 into harness scaffolding and route-native proof.
- Folded into plan: T8a, T8b, RP-13.

