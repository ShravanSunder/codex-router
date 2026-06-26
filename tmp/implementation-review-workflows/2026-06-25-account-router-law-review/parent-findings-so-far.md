# Parent Findings So Far

Date: 2026-06-25
Status: review lanes still running; these are parent-verified findings.

## Finding 1: Live state DB had schema drift not represented by current main migrations

Severity: blocker

Evidence:

- Live DB before reset had `PRAGMA user_version = 7`.
- Live DB also had `active_client_leases`.
- Current main source has no `active_client_leases` references in `crates/`.
- Current migration v7 is only `quota_snapshots.reset_credits_available`.
  See `crates/codex-router-state/src/sqlite.rs`.
- After moving the old DB aside and running current CLI, the new DB has
  `user_version = 7` and only:
  - `accounts`
  - `affinity_pins`
  - `previous_response_affinity_owners`
  - `quota_refresh_status`
  - `quota_snapshots`
  - `selector_quota_windows`

Scenario:

A branch or installed binary created `active_client_leases` without a distinct
schema version or migration contract. That makes quota/status/client-count
behavior depend on which binary last touched the DB, and current main cannot
reason about or clean that table.

Smallest fix:

- Decide whether active client leases belong in persisted state.
- If yes, add a proper next schema version and migration in
  `codex-router-state`, plus repository APIs and tests.
- If no, ensure no installed release path creates or reads that table, and
  remove stale CLI display dependencies.

Proof:

- Migration test from v7-without-active-client-leases to new version.
- Migration test from drifted v7-with-active-client-leases to expected current
  schema behavior, if we choose to recover old DBs.
- Fresh DB smoke showing no unsupported/drift tables.

## Finding 2: Release WebSocket path still enforces router-owned first-frame size policy

Severity: blocker

Evidence:

- `crates/codex-router-proxy/src/server.rs` constructs
  `WebSocketProtocolRouter::new(FirstFramePolicy::new(1024 * 1024))`.
- `crates/codex-router-proxy/src/websocket.rs` returns
  `WebSocketCloseReason::FirstFrameTooLarge` when the first text frame exceeds
  that policy.
- Live server log showed repeated:
  `websocket closed before upstream open: FirstFrameTooLarge`.

Scenario:

Real Codex first frames can exceed the router-owned cap. The router closes
before upstream open even though the payload is Codex-owned and should pass
through after account routing.

Smallest fix:

Replace close-on-large-payload behavior with bounded metadata extraction for
only account routing / affinity. If optional metadata cannot be extracted within
the bound, route without that optional metadata or fail only when required
account-routing metadata is truly unavailable.

Proof:

- Red/green test: large valid first Codex frame routes and is forwarded
  unchanged.
- Test: first-frame auth smuggling that is actually local-auth relevant remains
  rejected.
- Test: malformed/unknown Codex payload shape is not rejected unless routing
  cannot be performed.

## Live Reset Performed

Stopped running router PID:

- `77300 codex-router serve`

Moved old DB aside:

- `/Users/shravansunder/.codex-router/state.sqlite.backup-20260625-210712`

Fresh DB check:

- `PRAGMA user_version = 7`
- no `active_client_leases`
- `cargo run -q -p codex-router-cli -- account list` returned empty output.

Implication:

Accounts must be re-added before live routing can work from the fresh state DB.
