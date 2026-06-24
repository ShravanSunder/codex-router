# T4a Execution Receipt: Async WebSocket Tunnel Primitive

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T4a async WebSocket tunnel primitive
Starting HEAD: `e154e58`

## Scope Completed

- Added `AsyncWebSocketTunnel` beside the existing blocking tunnel.
- Kept existing first-frame/auth/selection/credential/audit logic in
  `AuthenticatedWebSocketRouter`.
- Used `tokio-tungstenite` for async upstream connection and async local
  `WebSocketStream` handling.
- Added a full-duplex async pump that forwards local-to-upstream and
  upstream-to-local messages concurrently after the first frame.
- Preserved upstream first-frame exact forwarding, sanitized upstream handshake
  headers, and top-level response-owner recording semantics.
- Aligned the proxy's direct `tungstenite` dependency with
  `tokio-tungstenite` at `0.29.0`.

## Red Evidence

- `cargo test -p codex-router-proxy async_websocket_tunnel -- --nocapture`
  - exit code: 101 before feature fix
  - expected failure: `tokio_tungstenite::connect_async` was gated behind the
    `connect` feature
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 101 before annotation
  - expected failure: test-only `accept_hdr_async` callback had the same
    `result_large_err` pattern already allowed on blocking WebSocket tests

## Green Evidence

- `cargo test -p codex-router-proxy async_websocket_tunnel -- --nocapture`
  - exit code: 0
  - result: 3 passed, 0 failed
- `cargo test -p codex-router-proxy -- --nocapture`
  - exit code: 0
  - result: 102 passed, 0 failed
- `cargo check --workspace`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo test --workspace -- --nocapture`
  - exit code: 0
  - result: all workspace unit/doc tests passed; `codex-router-test-support`
    still has 8 ignored smoke/e2e harness tests for later T7/T8 gates
- `cargo tree -p codex-router-proxy -i tungstenite@0.29.0`
  - exit code: 0
  - result: proxy direct dependency and `tokio-tungstenite` both resolve to
    `tungstenite 0.29.0`

## Matrix Status

- Supports T4/T5 WebSocket pump semantics and the missing full-duplex proof at
  the tunnel layer.
- Does not mark the release WebSocket rows passed yet because production
  `codex-router serve` still needs the Hyper upgrade and async tunnel wiring.

## Not Claimed

- Release `serve` does not yet call `AsyncWebSocketTunnel`.
- Local WebSocket upgrade is not yet owned by Hyper in production traffic.
- Revocation/session registry is still the blocking `TcpStream` registry.
- Three concurrent installed-Codex WebSocket clients have not been run yet.
