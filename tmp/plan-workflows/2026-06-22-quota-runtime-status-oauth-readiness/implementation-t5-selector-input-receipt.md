# T5 Durable Selector Input Receipt

Date: 2026-06-22
Worktree: `/Users/shravansunder/Documents/dev/open-source/ai-dev/codex-router.plan1a-quota-substrate-05bf755`
Branch: `plan1a-quota-substrate-05bf755`

## Scope

Implemented durable per-window selector input in router state and moved the
live quota-aware proxy selector implementation out of `http_sse.rs` into the
proxy account-selection adapter boundary.

## Implemented Rows

- 1A-15: per-window selector source is durable and selector adapter-owned.
- 1A-15a: selector crate remains state-free.
- 1A-15b: live selector adapter is mechanically moved out of HTTP/SSE.
- 1A-16: route-band selector state remains partitioned.

## Proof

- `cargo test -p codex-router-state tests::selector_input_reads_durable_per_window_rows_without_status_renderer -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo nextest run -p codex-router-state -- tests::selector_input_reads_durable_per_window_rows_without_status_renderer --exact`
  - exit 0; 1 passed, 8 skipped.
- `cargo test -p codex-router-state tests::quota_snapshots_are_partitioned_by_route_band_for_one_account -- --exact --list`
  - exit 0; listed exactly one test.
- `cargo nextest run -p codex-router-state -- tests::quota_snapshots_are_partitioned_by_route_band_for_one_account --exact`
  - exit 0; 1 passed, 8 skipped.
- `bash -lc 'set -euo pipefail; cargo tree -p codex-router-selection -e normal > /tmp/codex-router-selection-tree.txt; ! rg -n "codex-router-state" /tmp/codex-router-selection-tree.txt; cargo check -p codex-router-state -p codex-router-selection -p codex-router-proxy'`
  - exit 0; selector crate has no `codex-router-state` dependency and state/selection/proxy compile.
- `bash -lc 'set -euo pipefail; test -f crates/codex-router-proxy/src/account_selection.rs; ! rg -n -e "pub struct RepositoryBackedAccountSelector" -e "pub struct QuotaAwareAccountSelector" -e "impl.*RepositoryBackedAccountSelector" -e "impl.*QuotaAwareAccountSelector" crates/codex-router-proxy/src/http_sse.rs; rg -n -e "RepositoryBackedAccountSelector" -e "QuotaAwareAccountSelector" crates/codex-router-proxy/src/account_selection.rs crates/codex-router-proxy/src/server.rs; cargo check -p codex-router-proxy'`
  - exit 0; selector structs/impls live in `account_selection.rs`, server imports/constructs that adapter, and proxy compiles.
- `cargo nextest run -p codex-router-cli -- tests::serve_command_starts_runtime_and_forwards_one_loopback_request --exact`
  - exit 0; 1 passed, 38 skipped.
- `cargo nextest run -p codex-router-cli -- tests::serve_command_dispatches_websocket_upgrade_through_runtime --exact`
  - exit 0; 1 passed, 38 skipped.
- `cargo nextest run -p codex-router-cli -- tests::serve_command_reloads_token_rotation_without_restart --exact`
  - exit 0; 1 passed, 38 skipped.
- `cargo nextest run -p codex-router-state -p codex-router-proxy -p codex-router-cli`
  - exit 0; 92 passed, 0 skipped.
- `cargo clippy --workspace --all-targets -- -D warnings`
  - exit 0.
- `cargo fmt --all --check`
  - exit 0.

## Notes

- Runtime selection now reads `SelectorQuotaRepository` rows. Legacy quota
  snapshot rows remain intact for status/read models, while selector-facing
  rows carry durable per-window status, headroom, reset, limit-window seconds,
  effective marker, and route band.
- CLI/proxy runtime tests now seed selector windows explicitly so they prove the
  live durable selector input path instead of the removed request-time snapshot
  projection path.
