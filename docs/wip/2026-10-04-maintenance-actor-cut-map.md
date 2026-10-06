# Maintenance actor exact cut map

Planning checkpoint at HEAD `7a8cbd8943e6fb2dac23bcde7069203925003918`, branch `rust-file-decomposition`. This is an exact source map and completed-slice record; it is not a ready whole-work canonical plan.

## Ownership and scope

Fixes disposition `01a10438-ba74-7191-b287-1ac0321d5e92` permits pure reorganization of all 28 oversized files and private companions. The temporary fixes reservation for `crates/collaboration-mcp/src/mcp_server/tests.rs` was released by the fixes Lead; that file remains untouched. Host and selector clearances remain.

Allowed write set for this slice:

- `crates/codex-router-proxy/src/maintenance_actor.rs`
- new `crates/codex-router-proxy/src/maintenance_actor/maintenance_behavior_tests.rs`

No Cargo, CI, SQLx, migration, schema, public API, other crate, feature, tracing-target or production process writes.

## Parent source map

Before this cut, the root [maintenance_actor.rs](../../crates/codex-router-proxy/src/maintenance_actor.rs:1) had 1080 physical lines; after the applied cut it has 543. Keep production lines 1–539 unchanged, with one declaration replacement at the old inline-test boundary:

| Current lines | Responsibility | Action |
| ---: | --- | --- |
| 1–23 | imports and telemetry dependency | keep; `TestCompletionSender` remains parent-owned because production actor fields and server test helpers use it |
| 24–67 | public queue result and `MaintenanceHint` | keep public API and source-guarded declaration unchanged |
| 69–87 | test-only `MaintenanceCompletion` and accessors | keep in parent; `server.rs` and `proxy_tests` consume `crate::maintenance_actor::MaintenanceCompletion` |
| 89–200 | queued hint, coalescing key, repository boundary and SQLite adapter | keep unchanged |
| 202–398 | `MaintenanceActor` state/lifecycle/enqueue/shutdown | keep unchanged |
| 400–450 | `MaintenanceHint` classification/coalescing helpers | keep unchanged |
| 453–539 | `run_maintenance_actor` and lag helper | keep unchanged |
| 541–543 | inline `#[cfg(test)] mod tests {` opener | replace with `#[cfg(test)] #[path = "maintenance_actor/maintenance_behavior_tests.rs"] mod tests;` |
| 544–1079 | test imports, 12 tests, doubles and helpers | move as complete items to child |
| 1080 | inline module closing brace | remove with moved module wrapper |

The parent source guard test is one of the moved tests. Its `include_str!("maintenance_actor.rs")` must become `include_str!("../maintenance_actor.rs")`, because the child file is one directory below the production root; the string still reads the same production owner and searches the same `MaintenanceHint` and `MaintenanceRepository` markers.

## Child source map

Create `src/maintenance_actor/maintenance_behavior_tests.rs` containing old lines 544–1079 as complete items, preserving the following order and attributes:

- imports and `TEMP_COUNTER` (old 544–569); keep `use super::MaintenanceActor`, `MaintenanceEnqueueResult`, `MaintenanceHint`, `MaintenanceRepository`, `MaintenanceRepositoryError`; these names remain private-parent-visible through the child module.
- 12 tests, in existing order: `maintenance_hints_coalesce_without_request_admission_waiting` (old 571), `maintenance_hint_coalescing_does_not_emit_a_warning_log` (601), `stale_cleanup_hints_coalesce_by_route_band_and_class_without_cutoff_timestamp` (635), `session_affinity_cleanup_hints_coalesce_without_cutoff_timestamp` (665), `session_affinity_cleanup_hint_deletes_old_sqlite_rows` (690), `active_session_history_compaction_hint_deletes_completed_old_sqlite_events` (759), `coalesced_maintenance_remains_silent_after_waiting` (807), `maintenance_actor_shutdown_releases_sqlite_handle_without_socket_wait` (835), `maintenance_actor_shutdown_cancels_blocked_hint_within_threshold` (870), `maintenance_actor_degraded_enqueue_emits_scrubbed_lag_log` (895), `failed_maintenance_hint_is_degraded_then_allows_a_later_normal_hint` (935), `maintenance_actor_exposes_stale_cleanup_retention_and_compaction_hints` (966).
- test doubles and helpers: `BlockingMaintenanceRepository` (993), `FailingOnceMaintenanceRepository` (999), its `MaintenanceRepository` impl (1003), `wait_for_maintenance_call` (1022), blocking repository impl (1037), `refresh_rollups_hint` (1050), `retention_hint` (1059), `compaction_hint` (1066), `test_database_path` (1073).
- child ends at old line1079 after removing only the parent module’s final wrapper brace.

The existing 12 test identities, Tokio attributes, assertions, error canaries, SQL fixtures, counters, timeouts and cleanup remain unchanged. No test is removed, renamed or duplicated. The current module-qualified selector must be checked with `cargo test -p codex-router-proxy maintenance_actor:: -- --list`; that list must contain all 12 names.

## Callers and invariants

- `server.rs:625,635,650` and `proxy_tests/affinity_seed_fixtures.rs:75` use parent `MaintenanceCompletion`; retaining it at lines 69–87 avoids visibility or caller changes.
- `lib.rs:12` remains `pub mod maintenance_actor`; no public path changes.
- `server.rs:132–134,719–727,994,1038–1088` uses the actor/hints; production ownership does not move.
- The source guard continues to inspect production `maintenance_actor.rs`; only its relative path changes.
- New child module path follows the existing crate convention of named responsibility folders/files. No wrapper facade or second code path is introduced.

## Proof route

1. Before edit: bounded baseline `cargo check -p codex-router-state --locked` with PATH prefixed by `/opt/homebrew/opt/rustup/bin` and `CC/CXX/LDFLAGS/CPPFLAGS` unset: exit 0, finished dev profile in 0.37s. This proves the current pinned dependency/compiler path, not this cut.
2. Red/green structural checks after edit: `cargo test -p codex-router-proxy maintenance_actor:: -- --list` must show all12; `cargo test -p codex-router-proxy maintenance_actor::` exercises the moved scenarios and real SQLite cleanup.
3. Quality: `cargo fmt --all -- --check`; `cargo clippy -p codex-router-proxy --all-targets -- -D warnings`; package check.
4. Diff assertions: only the two allowed Rust paths; parent production lines/literals unchanged except test declaration and source-guard relative path; child physical size plus parent physical size both <=1000; compiled test inventory preserves all12.

Stop before editing if the child requires changing a public path, moving `MaintenanceCompletion`, changing test behavior/fixtures, or touching `mcp_server/tests.rs`.

## Applied result

Applied the map in the retained worktree. Parent production prefix lines1–539 remain byte-identical; parent now ends at 543 lines with the path child declaration. New child `maintenance_behavior_tests.rs` is 532 formatted lines. The child body compares equal to the original inline test body after formatting, with only the intentional `include_str!("../maintenance_actor.rs")` path change.

Validation completed after the final formatting correction:

- `cargo test -p codex-router-proxy maintenance_actor:: -- --list`: exit 0; all 12 named tests discovered.
- `cargo test -p codex-router-proxy maintenance_actor::`: exit 0; 12 passed, 0 failed, 476 filtered out, 0.06 seconds.
- `cargo fmt --all -- --check`: exit 0.
- `cargo check -p codex-router-proxy --locked`: exit 0, 0.26 seconds.
- `cargo clippy -p codex-router-proxy --all-targets --locked -- -D warnings`: exit 0, 0.23 seconds.
- Full repository size checker: exit 1 with 27 remaining oversized files; the maintenance actor is no longer listed.

No MCP test owner, CI, Cargo, SQLx, migration, public API, or production process changed. The whole decomposition and final CI integration remain incomplete.
