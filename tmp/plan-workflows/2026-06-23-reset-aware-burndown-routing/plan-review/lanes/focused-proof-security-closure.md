# Focused Plan Review Lane: Proof And Security Closure

Verdict: `needs revision`

## Accepted Findings

### Blocker: RP-11 had the wrong WebSocket failure boundary

- Problem: RP-11 said invalid first frames fail before local upgrade, but the
  accepted spec splits WebSocket failures:
  invalid local auth, unsupported paths, and forbidden subprotocol smuggling are
  non-101 pre-upgrade failures; malformed/wrong-type/oversized/timed-out/
  auth-smuggling/affinity-secret first-frame failures are post-upgrade,
  pre-upstream zero-side-effect failures.
- Plan edit: RP-11 and T6 now encode the split boundary and proof shape.

### Blocker: grouped cargo test filters were not exact selectors

- Problem: `cargo test <filter>` matches test names containing the filter string,
  so route-native and installed-Codex grouped commands need inventory proof.
- Plan edit: T8a, T8b, T9, T10, and T11 now require `--list` preflight receipts
  proving matched test names/counts are exactly the intended grouped suites
  before running the grouped ignored proof commands.

### Important: redaction missed affinity secret-store identifiers

- Problem: RP-15/T11 mentioned affinity secrets but did not explicitly include
  the stable secret-store key name, backend identifiers, or derived secret
  material.
- Plan edit: RP-15/T11 now include
  `router_affinity_hash_secret.v1`, secret-store backend identifier canaries,
  and derived-secret-material canaries in negative scans.

### Nit: prior review ledger count was stale

- Problem: plan coverage said the prior review ledger was 67 lines; it is 68.
- Plan edit: source coverage now says 68 lines.

## Closure Checklist

- Owner-record writes: closed.
- HTTP/SSE affinity-secret ordering: closed.
- WebSocket auth coverage: closed.
- T2 split: closed.
- T8 split: closed.
- T5/T6 unsafe parallelism: closed.
- T7/T11 allowlists: closed after T7 serialization.
- Secret-loss recovery: closed.
- Installed-Codex transport isolation: closed with exact commands plus inventory
  preflight.
- Final validation: closed with deny/audit plus expanded redaction canaries.

