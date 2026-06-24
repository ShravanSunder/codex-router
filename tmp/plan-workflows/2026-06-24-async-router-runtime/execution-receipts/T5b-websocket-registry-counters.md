# T5b WebSocket Registry Counters Receipt

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T5b registry active/high-water/cleanup counters

## Scope

- Added redacted WebSocket registry counters for active sessions, high-water
  sessions, registered sessions, and closed sessions.
- Changed async WebSocket session registration to return a lifetime guard so
  counters are decremented when the forwarding session exits.
- Kept token-generation revocation semantics: stale generation cancellation
  tokens are cancelled, active generation tokens are left alone.
- Added direct registry unit tests and async tunnel integration assertions.

## Evidence

Commands run from repo root:

```text
cargo fmt --all -- --check
exit 0
```

```text
cargo clippy --workspace --all-targets -- -D warnings
exit 0
```

```text
cargo test -p codex-router-proxy registry_ -- --nocapture
exit 0
result: 2 passed
```

```text
cargo test -p codex-router-proxy async_websocket_tunnel_forwards_first_frame_and_second_local_frame -- --nocapture
exit 0
result: 1 passed
```

## Matrix Rows Advanced

- I-15: advanced by direct active/high-water/zero-active registry cleanup proof.
- I-14: maintained by stale-generation revocation proof.
- E-06: partially advanced by router-side registry high-water/zero-active
  counters in the async tunnel path. Child-process artifact export of these
  counters remains open.

## Remaining Gates

- Child-process installed-Codex transcript still does not read router registry
  counters from the `serve` process.
- E-02 five-minute soak remains open.
- T5 blocked-write/backpressure cleanup and T6 final structural guardrails
  remain open.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T5b-websocket-registry-counters.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Router-side WebSocket registry counters and cleanup semantics are proven in unit and async tunnel tests; continue toward child-process registry export, soak, and structural guardrails.
