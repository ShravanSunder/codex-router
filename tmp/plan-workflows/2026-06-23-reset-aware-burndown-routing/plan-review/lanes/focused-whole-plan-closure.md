# Focused Plan Review Lane: Whole-Plan Closure

Verdict: `needs revision`

## Accepted Findings

### Blocker: T5 checkpoint still implied WebSocket ingress proof

- Problem: T5 owns shared local-auth and HTTP/SSE files, but proof language could
  be read as requiring WebSocket ingress, non-101, subprotocol, and call-counter
  proof that T6 owns.
- Plan edit: T5 now proves only shared local-auth primitive behavior plus
  HTTP/SSE-owned security paths. T6 owns WebSocket ingress, non-101,
  subprotocol, and call-counter proof. T5 checkpoint wording explicitly excludes
  T6 WebSocket ingress proof.

### Important: T7 still overlapped T6

- Problem: T7 could run beside T6 even though both could edit router/test-support
  WebSocket surfaces and T7's WebSocket proof depends on the final T6 path.
- Plan edit: T7 is now strictly after T6. Non-blocking WebSocket assertions must
  run against the final T6 WebSocket ingress, affinity, and pinning path.

## Closure Checklist

- Previous-response owner writes: closed.
- HTTP/SSE affinity-secret fail-closed ordering: closed.
- WebSocket local-auth coverage: closed.
- T8 split: closed.
- Installed-Codex transport isolation: closed.
- T2 split: closed.
- Status UX strictness: closed.
- T5/T6 parallelism: closed.
- T7/T11 write scopes: closed after T7 serialization.
- Secret-loss/replacement recovery: closed.
- Final validation deny/audit/redaction scans: closed subject to proof/security
  secret-identifier expansion.

