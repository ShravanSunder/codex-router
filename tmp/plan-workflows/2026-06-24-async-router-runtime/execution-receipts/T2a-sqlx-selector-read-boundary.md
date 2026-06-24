# T2a Execution Receipt: SQLx Selector Read Boundary

Date: 2026-06-24
Goal id: `2026-06-24-async-router-runtime`
Slice: T2a async request-time selector state reads
Starting HEAD: `d8fd37f`

## Scope Completed

- Added SQLx SQLite dependency using one native SQLite link path.
- Downgraded temporary `rusqlite` bridge from `0.40.1` to `0.37.0` so
  `rusqlite` and `sqlx 0.9.0` resolve to compatible `libsqlite3-sys` versions.
- Added `AsyncSqliteStateStore` backed by `sqlx::SqlitePool`.
- Added async read-only selector contracts:
  - `AsyncSelectorQuotaRepository::selector_inputs_for_route_band`
  - `AsyncAffinityRepository::load_previous_response_owner`
- Mirrored current sync selector projection semantics:
  - deterministic account ordering
  - selector-window order by effective window and limit
  - stale overlay from refresh status
  - previous-response owner missing/found/ambiguous mapping

## Red Evidence

- `cargo test -p codex-router-state async_selector_input_matches_sync_repository_projection -- --nocapture`
  - exit code: 101 before implementation
  - expected failure: missing `AsyncSqliteStateStore`,
    `AsyncSelectorQuotaRepository`, and Tokio test dependency
- `cargo test -p codex-router-state async_previous_response_affinity_owner_matches_sync_repository_projection -- --nocapture`
  - exit code: 101 before implementation
  - expected failure: missing `AsyncAffinityRepository`
- Intermediate dependency red:
  - `sqlx 0.8.6` / `sqlx 0.9.0` conflicted with `rusqlite 0.40.1` over
    incompatible `libsqlite3-sys` `links = "sqlite3"` versions.

## Green Evidence

- `cargo test -p codex-router-state async_ -- --nocapture`
  - exit code: 0
  - result: 2 passed, 0 failed
- `cargo test -p codex-router-state -- --nocapture`
  - exit code: 0
  - result: 21 passed, 0 failed
- `cargo check --workspace`
  - exit code: 0
- `cargo fmt --all -- --check`
  - exit code: 0
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit code: 0

## Matrix Status

- This supports T2 row `U-06` and the request-time read portion of T2, but does
  not mark `U-06` passed yet because `scripts/proof-matrix.sh U-06` is still a
  pending scaffold row.
- `I-16`, `U-07`, credential commit failpoints, proxy runtime async call-site
  cutover, and SQLx writes remain open.

## Not Claimed

- Proxy runtime does not yet call the async state contracts.
- Credential refresh commit is not yet converted to async SQLx.
- Runtime request path is not yet free of direct sync `rusqlite`.
- Release `serve` is not yet async-complete.
