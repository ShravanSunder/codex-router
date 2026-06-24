# T1 Execution Receipt: Async Runtime Substrate

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T1 single async production serve substrate and proof scaffold
Starting HEAD: `9fbd57e`

## Scope Completed

- Added workspace async/proxy dependency substrate:
  - Tokio
  - tokio-util
  - tokio-tungstenite
  - Hyper
  - hyper-util
  - hyper-rustls
  - http/http-body-util/bytes/futures-util
- Added proxy-owned `AsyncLoopbackServerRuntime` substrate:
  - Tokio loopback bind
  - cancellation-token controlled accept shell
  - no release `serve` cutover claim
- Added proxy-owned `HyperProtocolSwitchpoint`:
  - classifies Hyper request heads into HTTP vs WebSocket upgrade
  - does not consume or buffer request bodies
- Added `scripts/proof-matrix.sh` scaffold:
  - known pending rows write allowlisted receipts and exit 3
  - unknown rows exit 2
  - no rows are marked passed by scaffold alone
- Added implementation-execute controller brief.

## Red Evidence

- `cargo test -p codex-router-proxy hyper_protocol_switchpoint -- --nocapture`
  - exit code: 101 before implementation
  - expected failure: unresolved `AsyncLoopbackServerRuntime`,
    `HyperProtocolDispatch`, `HyperProtocolSwitchpoint`, `tokio`,
    `tokio_util`, and `http`
- `test -x scripts/proof-matrix.sh && scripts/proof-matrix.sh I-21`
  - exit code: 1 before implementation
  - expected failure: proof-matrix command missing

## Green Evidence

- `cargo test -p codex-router-proxy async_loopback_server_binds_ephemeral_listener_and_stops_on_cancellation -- --nocapture`
  - exit code: 0
  - result: 1 passed, 0 failed
- `cargo test -p codex-router-proxy hyper_protocol_switchpoint -- --nocapture`
  - exit code: 0
  - result: 2 passed, 0 failed
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo check --workspace`
  - exit code: 0
- `scripts/proof-matrix.sh NOT-A-ROW`
  - observed exit code through wrapper: 2
  - result: unknown row rejected
- `scripts/proof-matrix.sh I-21`
  - observed exit code through wrapper: 3
  - result: known row remains `pending_unimplemented`
  - receipt:
    `tmp/plan-workflows/2026-06-24-async-router-runtime/evidence/integration/I-21.json`

## Matrix Status

- No proof-matrix row is marked passed by T1 scaffold alone.
- `I-21` is still pending and must be implemented by the startup/slow-refresh
  harness work before it can become green.

## Not Claimed

- Release `codex-router serve` is not yet async-complete.
- HTTP/SSE proxying is not yet cut over to Hyper.
- WebSocket upgrade/pumps are not yet cut over to Hyper + tokio-tungstenite.
- SQLx state/auth boundary is not yet implemented.
