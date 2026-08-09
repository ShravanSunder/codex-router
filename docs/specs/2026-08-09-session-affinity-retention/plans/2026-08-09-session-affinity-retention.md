# Session-affinity retention implementation plan

Planning result: draft

> **For agentic workers:** REQUIRED SUB-SKILL: use test-driven development and
> execute each task against the current isolated worktree.

**Goal:** Delete session-account affinities older than seven days through the
existing bounded maintenance path at most once per UTC day.

**Architecture:** `LoopbackRouterRuntime` owns a process-local UTC-day attempt
guard and submits one global `MaintenanceHint`. `MaintenanceActor` dispatches
the hint to `AsyncSqliteStateStore`, which uses an idempotently ensured
timestamp index and one asynchronous delete. No request awaits maintenance.

**Tech stack:** Rust 2024, Tokio, SQLx SQLite, existing proxy/state test
harnesses.

## Global constraints

- Follow the approved specification at
  `docs/specs/2026-08-09-session-affinity-retention/2026-08-09-session-affinity-retention.md`.
- Use explicit parameter and return types and descriptive names.
- Do not block inside async code or add a task, timer, service, configuration,
  schema-version bump, or persisted cleanup schedule.
- Preserve unrelated work.

## Task 1: Indexed state cleanup

**Files:**

- Modify `crates/codex-router-state/src/sqlite.rs`.
- Modify `crates/codex-router-state/src/lib.rs`.

**Produces:**

- `AsyncSqliteStateStore::purge_session_account_affinities_before(cutoff_unix_seconds: u64) -> Result<(), StateStoreError>`.
- An idempotent `session_account_affinities(last_seen_unix_seconds)` index on
  writable open without a schema-version change.

- [ ] Add a state test that seeds old, exact-cutoff, and fresh rows and fails
      because the purge API is absent.
- [ ] Run the focused test and verify the expected compile failure.
- [ ] Add the minimal async delete and index ensure path.
- [ ] Run focused state tests and verify old-only deletion plus unchanged v13.

## Task 2: Daily maintenance dispatch

**Files:**

- Modify `crates/codex-router-proxy/src/maintenance_actor.rs`.
- Modify `crates/codex-router-proxy/src/server.rs`.

**Produces:**

- Global `MaintenanceHint::CleanupStaleSessionAccountAffinities` carrying one
  cutoff.
- Process-local UTC-day attempt guard.

- [ ] Add failing maintenance dispatch/coalescing tests.
- [ ] Add failing pure guard tests for first attempt, same UTC day, next UTC
      day, and clock rollback.
- [ ] Run focused proxy tests and verify the expected failures.
- [ ] Implement the hint, repository dispatch, seven-day cutoff, and guard.
- [ ] Run focused proxy tests and verify the request path remains non-awaiting.

## Task 3: Integrated proof

- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] Run `cargo test --workspace -- --test-threads=1`.
- [ ] Seed an isolated SQLite database, trigger cleanup through the runtime
      maintenance seam, and prove old-only deletion without touching production.
- [ ] Obtain a fresh independent implementation review against the exact diff
      and proof identities before PR publication.
