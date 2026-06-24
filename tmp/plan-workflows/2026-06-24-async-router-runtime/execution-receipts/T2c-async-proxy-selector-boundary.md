# T2c Execution Receipt: Async Proxy Selector Boundary

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T2c async proxy account-selection boundary
Starting HEAD: `92fe793`

## Scope Completed

- Added `AsyncAccountDecisionSelector` for Tokio runtime callers.
- Added `AsyncRepositoryBackedAccountSelector` over the SQLx-backed async
  selector and affinity repository contracts.
- Mirrored existing repository-backed selector behavior for:
  - route-band-specific selector quota reads
  - previous-response affinity owner routing
  - process-local account hold cooldown
- Kept repository awaits before acquiring process-local weighted-selector and
  account-hold mutexes.
- Kept the existing sync selector path intact for the current blocking runtime.

## Red Evidence

- `cargo test -p codex-router-proxy async_repository_backed_selector -- --nocapture`
  - exit code: 101 before implementation
  - expected failure: missing `AsyncAccountDecisionSelector` and
    `AsyncRepositoryBackedAccountSelector`
- Intermediate compiler gate:
  - exit code: 101
  - required `R: Sync` bound for the `Send` boxed async selector future

## Green Evidence

- `cargo test -p codex-router-proxy async_repository_backed_selector -- --nocapture`
  - exit code: 0
  - result: 3 passed, 0 failed
- `cargo test -p codex-router-proxy repository_backed_selector -- --nocapture`
  - exit code: 0
  - result: 20 passed, 0 failed
- `cargo check --workspace`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo test -p codex-router-proxy -- --nocapture`
  - exit code: 0
  - result: 99 passed, 0 failed

## Matrix Status

- This supports the async selector portion of T2 and the lock-ordering
  prerequisite for multi-client runtime work.
- It does not mark the full async runtime path passed because the release
  `serve` call path still needs the Hyper/tokio-tungstenite cutover and
  multi-client e2e proof.

## Not Claimed

- Release `serve` does not yet use the async selector.
- The WebSocket tunnel is not yet converted to `tokio-tungstenite`.
- The loopback server is not yet Hyper-backed for production traffic.
- The three concurrent real-Codex WebSocket proof gate has not run yet.
