# T4b Execution Receipt: Hyper WebSocket Upgrade Helper

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T4b Hyper WebSocket upgrade response helper
Starting HEAD: `e6c9375`

## Scope Completed

- Added `HyperWebSocketUpgrade::switching_protocols_response`.
- Computes `Sec-WebSocket-Accept` with tungstenite's handshake utility while
  keeping Hyper as the owner of the local `101 Switching Protocols` response.
- Added explicit failure for missing or invalid `Sec-WebSocket-Key`.
- Added protocol ownership tests near the existing Hyper switchpoint tests.

## Red Evidence

- `cargo test -p codex-router-proxy hyper_ -- --nocapture`
  - exit code: 101 before fixes
  - expected failures:
    - unqualified `Error` derive in `server.rs`
    - direct `Result<Response<()>, _>` equality in the missing-key test even
      though `http::Response<()>` does not implement `PartialEq`

## Green Evidence

- `cargo test -p codex-router-proxy hyper_ -- --nocapture`
  - exit code: 0
  - result: 4 passed, 0 failed
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0
- `cargo fmt --all -- --check`
  - exit code: 0 after formatting

## Matrix Status

- Supports T4 local Hyper upgrade ownership and the guardrail requirement that
  the production path must not perform a second local tungstenite accept
  handshake after Hyper upgrades the stream.

## Not Claimed

- Production `serve` does not yet call the Hyper upgrade helper.
- `hyper::upgrade::on` is not wired to `AsyncWebSocketTunnel` yet.
- No installed-Codex or multi-client runtime proof is claimed by this slice.
