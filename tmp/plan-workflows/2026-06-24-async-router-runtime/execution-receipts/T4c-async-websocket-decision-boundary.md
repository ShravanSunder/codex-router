# T4c Execution Receipt: Async WebSocket Decision Boundary

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T4c async WebSocket first-frame selection and credential boundary
Starting HEAD: `226deaa`

## Scope Completed

- Added `AsyncProviderCredentialResolver` for async WebSocket runtime callers.
- Added `AsyncAuthenticatedWebSocketRouter` so local auth, first-frame
  validation, account selection, provider credential resolution, header
  sanitation, audit events, and affinity-owner context all compose through async
  request-time contracts.
- Changed `AsyncWebSocketTunnel` to use `AsyncAuthenticatedWebSocketRouter`
  instead of the sync `AuthenticatedWebSocketRouter`.
- Added async selector and provider credential test doubles.
- Added async first-frame routing proof that selection happens after local auth
  and first-frame validation, and credential resolution happens only for the
  selected account.
- Updated async tunnel tests to use the async selector and credential boundary.

## Red Evidence

- `cargo fmt --all -- --check`
  - exit code: 1 before formatting
  - expected failure: new imports and assertion formatting did not match
    rustfmt ordering.

## Green Evidence

- `cargo test -p codex-router-proxy async_ -- --nocapture`
  - exit code: 0
  - result: 8 passed, 0 failed
- `cargo test -p codex-router-proxy -- --nocapture`
  - exit code: 0
  - result: 105 passed, 0 failed
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo check --workspace`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0
- `cargo test --workspace -- --nocapture`
  - exit code: 0
  - result: all workspace unit/doc tests passed; `codex-router-test-support`
    still has 8 ignored smoke/e2e harness tests reserved for T7/T8.

## Matrix Status

- Supports T4/T5 async WebSocket selection, credential, and first-frame
  boundary work.
- Supports U-02, U-03, U-05, U-06, U-07, I-20, and G-23 design intent at the
  unit/integration helper layer.
- Does not mark matrix rows `[x] passed` yet because `scripts/proof-matrix.sh
  <ROW>` row artifacts and release-serve reachability proof still need to be
  created by later slices.

## Not Claimed

- Production `codex-router serve` still does not call the Hyper upgrade helper
  or `AsyncWebSocketTunnel`.
- `hyper::upgrade::on` is not wired to the async tunnel yet.
- The release path still needs T3/T4/T5 composition before any installed-Codex
  or multi-client runtime proof can be claimed.
- Session registry, revocation, and close semantics are still pending T5.
