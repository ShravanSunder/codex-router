# T7 Real Serve Installed-Codex Smoke Receipt

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T7a/T7b child-process `codex-router serve` smoke harness

## Scope

- Replaced the installed-Codex smoke harness router path with a spawned built
  `target/debug/codex-router serve` child process.
- Added `serve --audit-file <path>` so real child-process smoke can keep the
  same local-auth audit proof previously available only through the in-process
  runtime helper.
- Made the deterministic mock upstream tolerate multiple WebSocket sessions
  and preserve the combined HTTP/SSE plus best non-prewarm WebSocket transcript.
- Updated `tests/smoke/installed_codex_mock.sh` to build the router binary
  before running ignored installed-Codex tests.
- Updated workflow-state details so the latest accepted transition points to
  `shravan-dev-workflow:implementation-execute-plan`.

## Evidence

Commands run from repo root:

```text
cargo fmt --all -- --check
exit 0
```

```text
cargo check -p codex-router-test-support
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
cargo test -p codex-router-test-support installed_codex_websocket_harness_inventory_preflight -- --ignored --nocapture
exit 0
result: 1 passed
```

```text
tests/smoke/installed_codex_mock.sh --transport websocket
exit 0
result: 2 passed
```

```text
tests/smoke/installed_codex_mock.sh --transport http-sse
exit 0
result: 2 passed
```

```text
tests/smoke/installed_codex_mock.sh --transport all
exit 0
result: 6 passed
```

```text
cargo test -p codex-router-test-support installed_codex::tests -- --nocapture
exit 0
result: 5 passed, 7 ignored
```

```text
cargo test --workspace -- --nocapture
exit 0
result: 263 passed, 8 ignored
```

Newest smoke transcripts inspected by modification time:

```text
tmp/smoke/installed-codex-mock-58883-1782312729182.json
mode=websocket
spawned_real_serve_child=true
binary=/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router/target/debug/codex-router
pid=59355
listener=127.0.0.1:53793
argv includes: serve --port 53793 --listen-host 127.0.0.1 --state-db ... --secret-root ... --upstream-base-url http://127.0.0.1:53792/v1 --disable-background-quota-refresh --max-connections 64 --audit-file ...
cleanup=terminated:signal: 9 (SIGKILL)
websocket request frames=1
```

```text
tmp/smoke/installed-codex-mock-58883-1782312728481.json
mode=combined
spawned_real_serve_child=true
pid=59134
listener=127.0.0.1:53766
http_sse.ran=true
websocket.request_frame_count=1
```

## Matrix Rows Advanced

- S-01: advanced by installed-Codex tokenless default profile smoke through
  child `codex-router serve`.
- S-03: advanced by transcript fields for binary path, PID, argv, listener,
  readiness, and cleanup.
- S-04: advanced by deterministic mock upstream WebSocket/HTTP smoke through
  real child `serve`.

Rows are not marked globally complete in the final matrix yet because
`scripts/proof-matrix.sh <ROW>`, aggregate redaction scan, stale-artifact scan,
T8 three-runtime e2e, T8 five-minute soak, and final T6c structural guardrails
remain open.

## Remaining Gates

- T3 pure Hyper HTTP/SSE upstream path still uses the temporary blocking bridge.
- T5 registry/close/backpressure semantics are not fully proven by this receipt.
- T6 release-reachability guardrails are not complete.
- T8 three concurrent installed Codex runtimes and five-minute overlap soak are
  still open.

phase_result: complete
evidence: `tmp/plan-workflows/2026-06-24-async-router-runtime/execution-receipts/T7-real-serve-installed-codex-smoke.md`
recommended_next_workflow: `shravan-dev-workflow:implementation-execute-plan`
recommended_transition_reason: T7 real child-process installed-Codex smoke proof is green; continue implementation toward T8 concurrency/soak and remaining T3/T5/T6 hard gates.
