# DB write actor decomposition cut map

Bounded mechanics-only decomposition under the owner continuation instruction. The cut separates session-affinity scheduling and the large actor test harness from production queue/command handling.

## Write set

- `crates/codex-router-proxy/src/db_write_actor.rs`
- new `crates/codex-router-proxy/src/db_write_actor/session_affinity_scheduler.rs`
- new `crates/codex-router-proxy/src/db_write_actor/test_support.rs`
- new `crates/codex-router-proxy/src/db_write_actor/queue_tests.rs`
- new `crates/codex-router-proxy/src/db_write_actor/session_affinity_tests.rs`

## Exact map

Before the cut, `db_write_actor.rs` had 2817 physical lines. Move the session-affinity scheduler helpers (`await_db_write_task_shutdown`, trigger processing, debounce/max-wait flush and persistence) into the private production sibling `session_affinity_scheduler.rs` (192 lines). Move the inline test fixtures into `test_support.rs` (348 lines), queue-pressure/degraded-health tests into `queue_tests.rs` (758 lines), and session-affinity/shutdown tests into `session_affinity_tests.rs` (540 lines). The parent is 998 formatted lines and keeps all public types, repository/actor implementations, queue dispatch, and command result handling.

Test fixtures and scheduler functions use `pub(super)` seams only. No DB schema, persistence format, queue semantics, telemetry, auth/security, CI, or production behavior changes.

## Proof

- `cargo test -p codex-router-proxy db_write_actor --lib`: exit 0; 28 passed, 0 failed.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-proxy --locked`: exit 0.
- `cargo clippy -p codex-router-proxy --all-targets --locked -- -D warnings`: exit 0.
- Strict size checker: exit 1 with 3 remaining oversized Rust files; `db_write_actor.rs` is no longer listed.

This checkpoint is intentionally uncommitted until the five Rust paths, this map, and the work trace are staged together. No CI integration or final workspace gate is included in this slice.
