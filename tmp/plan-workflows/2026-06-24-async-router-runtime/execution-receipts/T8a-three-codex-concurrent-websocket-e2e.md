# T8a Three-Codex Concurrent WebSocket E2E Receipt

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T8a bounded three-client WebSocket overlap proof

## Scope

- Added a WebSocket-only concurrent installed-Codex e2e harness.
- The harness starts one built `codex-router serve` child process, then starts
  three isolated installed `codex` child processes against the same router port.
- Added a deterministic concurrent WebSocket mock upstream that handles each
  accepted WebSocket on its own thread and gates non-prewarm response completion
  until the upstream observes all three active non-prewarm sessions.
- Added `tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent`.
- Kept the previous serial smoke as the default `--transport all` six-test
  path.

## Evidence

Commands run from repo root:

```text
cargo fmt --all -- --check
exit 0
```

```text
cargo check --workspace
exit 0
```

```text
cargo clippy --workspace --all-targets -- -D warnings
exit 0
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket --scenario concurrent
exit 0
result: 1 passed
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit 0
result: 6 passed
```

```text
cargo test --workspace -- --nocapture
exit 0
result: 263 passed, 9 ignored
```

Newest three-client transcript inspected by modification time:

```text
tmp/smoke/installed-codex-three-websocket-81932-1782313296386.json
mode=three-websocket
clients=3
all_success=true
router_pid=81994
spawned_real_serve_child=true
listener=127.0.0.1:59402
expected_sessions=3
completed_sessions=3
active_high_water=3
overlap_proven=true
session_frame_counts=[1, 1, 1]
```

## Matrix Rows Advanced

- E-01: advanced by three installed Codex child processes sharing one recorded
  router PID and completing through WebSocket.
- E-03: advanced by upstream barrier/high-water evidence proving all three
  non-prewarm WebSocket sessions overlapped before completion.
- E-04: partially advanced for frame activity per runtime; the current bounded
  e2e proves one non-prewarm exchange per runtime, while the plan's three
  post-handshake interactions per runtime remains open for the soak/deeper T8
  row.
- E-06: partially advanced by upstream high-water 3. Router registry high-water
  evidence is still open.
- E-07: partially advanced by shared router PID and upstream/client counts.

## Remaining Gates

- E-02 five-minute soak remains open.
- E-04 still requires three post-handshake interactions or frame exchanges per
  runtime during the overlap window.
- E-05 deterministic tool-call-style/multi-step interleave remains open.
- E-06 router registry high-water and zero-active-after evidence remains open.
- E-08 socket cleanup checker remains open.
- E-09 aggregate allowlist redaction validator remains open.
- T3 pure Hyper HTTP/SSE upstream path, T5 registry/backpressure cleanup, and
  T6 final structural guardrails remain open.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T8a-three-codex-concurrent-websocket-e2e.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: Bounded three-client installed-Codex WebSocket overlap proof is green; continue implementing remaining T8 soak/deeper interaction and T6/T5/T3 hard gates.
