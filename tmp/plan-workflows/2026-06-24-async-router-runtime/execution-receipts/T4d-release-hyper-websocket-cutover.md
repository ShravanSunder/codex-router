# T4d Execution Receipt: Release Hyper WebSocket Cutover

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T4d release `serve` Hyper WebSocket cutover with async revocation
Starting HEAD: `5fe412f`

## Scope Completed

- Changed production `LoopbackRouterRuntime::serve_protocol_connections` to run
  a Tokio HTTP/1 server loop over the existing loopback listener.
- Local HTTP/1 parsing and WebSocket upgrade dispatch now enter through Hyper
  for the production `serve` path.
- Hyper owns local `101 Switching Protocols`; upgraded local streams are wrapped
  with `TokioIo<Upgraded>` and
  `tokio_tungstenite::WebSocketStream::from_raw_socket(..., Role::Server, ...)`.
- Production WebSocket sessions now use `AsyncWebSocketTunnel`,
  `AsyncRepositoryBackedAccountSelector`, `AsyncSqliteStateStore`, and the
  async credential resolver bridge.
- Enabled Hyper HTTP/1 half-close support so local clients that shutdown their
  write side after one request still receive a response.
- Added async token-rotation revocation via shared cancellation tokens in
  `WebSocketRevocationRegistry`.
- Preserved pre-upgrade local-auth, subprotocol smuggling, unsupported route,
  audit, affinity-owner, and account-hold behavior through the release path.

## Red Evidence

- `cargo test -p codex-router-proxy loopback_router_runtime_ -- --nocapture`
  - exit code: 101 before fixes
  - expected failures after first Hyper cutover:
    - bounded test runtime dropped upgraded WebSocket tasks too early
    - audit sink was not connected to the async production tunnel
    - local clients that half-closed write side saw empty HTTP responses until
      Hyper half-close support was enabled
    - token rotation reached async clients as a close frame, requiring the test
      to assert explicit WebSocket close as valid closure
- `cargo test -p codex-router-cli serve_command -- --nocapture`
  - exit code: 101 before close-frame assertion update
  - expected failure: CLI rotation smoke also saw a close frame instead of the
    old blocking socket reset.

## Green Evidence

- `cargo test -p codex-router-proxy loopback_router_runtime_ -- --nocapture`
  - exit code: 0
  - result: 15 passed, 0 failed
- `cargo test -p codex-router-proxy async_websocket_tunnel -- --nocapture`
  - exit code: 0
  - result: 3 passed, 0 failed
- `cargo test -p codex-router-cli serve_command -- --nocapture`
  - exit code: 0
  - result: 6 passed, 0 failed
- `cargo test -p codex-router-cli served_router -- --nocapture`
  - exit code: 0
  - result: 2 passed, 0 failed
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo check --workspace`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0
- `cargo test --workspace -- --nocapture`
  - exit code: 0
  - result: all workspace unit/doc tests passed; `codex-router-test-support`
    still has 8 ignored smoke/e2e harness tests reserved for later T7/T8.

## Matrix Status

- Strongly advances T4 release local-upgrade ownership and T5 revocation/duplex
  proof at the unit/integration release-path level.
- Existing permanent tests now cover:
  - second WebSocket accepted while first is blocked
  - HTTP accepted while WebSocket is blocked
  - fragmented WebSocket upgrade through release path
  - unsupported/local-auth/smuggling pre-upgrade rejection
  - token rotation closes stale async WebSocket sessions
- Does not mark matrix rows `[x] passed` because most
  `scripts/proof-matrix.sh <ROW>` row harnesses still emit pending scaffold
  receipts.

## Not Claimed

- HTTP/SSE upstream transport is not yet pure Hyper; the release HTTP path
  temporarily bridges through the existing blocking HTTP service in
  `spawn_blocking`.
- The private legacy blocking protocol helper still exists for comparison and
  later T6 removal; it is no longer called by `serve_protocol_connections`.
- No installed-Codex child-process smoke/e2e or three-client soak has been run.
- Structural guardrails G-01 through G-23 are not complete.
