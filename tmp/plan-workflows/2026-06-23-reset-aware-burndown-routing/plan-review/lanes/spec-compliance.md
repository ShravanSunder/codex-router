# Plan Review Lane: Spec Compliance

Verdict: `needs revision`

## Accepted Findings

### Blocker: HTTP/SSE affinity-secret gate was missing

- Problem: T3 consumed and produced previous-response affinity but did not
  explicitly load/create `router_affinity_hash_secret.v1` before selector
  advancement for HTTP/SSE response-capable routes.
- Failure: HTTP/SSE could select credentials or open upstream when the affinity
  secret is unavailable.
- Required edit: add HTTP/SSE `affinity_secret_unavailable` fail-closed ordering
  and zero-side-effect proof.
- Folded into plan: T3, RP-09, RP-10.

### Blocker: WebSocket local-auth coverage was incomplete

- Problem: the plan missed WebSocket manual `X-Codex-Router-Token`, mismatched
  mixed-carrier equality, and `Sec-WebSocket-Protocol` token smuggling.
- Failure: WebSocket could authorize a mixed or forbidden carrier before the
  local upgrade.
- Required edit: make local-auth shared across transports and add WebSocket
  ingress matrix rows.
- Folded into plan: T5, T6, RP-06, RP-11.

### Important: Installed-Codex WebSocket proof was weaker than the spec

- Problem: T10 did not require status agreement or upstream WebSocket
  local-auth stripping proof.
- Required edit: add both assertions to WebSocket e2e.
- Folded into plan: T10, RP-14.

### Important: Status proof matrix was underspecified

- Problem: T4 did not force the UI cases that caused prior confusion.
- Required edit: require account-centric rows, default responses route only,
  healthy/partial/unknown/blocked/reserve snapshots, Unicode bars, and negative
  wording checks.
- Folded into plan: T4, RP-12.

