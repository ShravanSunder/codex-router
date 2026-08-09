# Session-affinity retention

## Goal

Bound growth of routing-owned session-affinity state without adding a cleanup
service, timer task, configuration surface, or persisted scheduler state.

## Required behavior

- Retain `session_account_affinities` for seven days after
  `last_seen_unix_seconds`.
- Delete rows only when `last_seen_unix_seconds < now - 7 days`; a row exactly
  at the cutoff remains until a later cleanup.
- Run cleanup through the existing bounded `MaintenanceActor` and its dedicated
  SQLite maintenance pool.
- Request cleanup once at router startup and at most once per UTC day on later
  accepted connections.
- Keep the daily guard process-local. A router restart may harmlessly request
  cleanup again on the same day.
- Record the UTC day as attempted before enqueueing. A full/closed maintenance
  queue or repository failure is best-effort and retries on the next UTC day,
  not on every later connection that day.
- Socket handling and account selection must never await cleanup, and cleanup
  failure must not affect routing health. SQLite still serializes writes to the
  shared database file.

## Scope boundaries

- Do not change the existing seven-day quota-history purge.
- Do not delete previous-response affinity owners, active-session events,
  accounts, routing policies, quota snapshots, selector state, refresh state,
  route-band account state, or explicit affinity pins.
- Do not add a background service, periodic task, configuration option,
  schema-version migration, cleanup-state table, distributed lease, or
  production-process action.

## Ownership and flow

1. `LoopbackRouterRuntime` decides whether the current UTC day has already
   requested session-affinity cleanup.
2. The existing `MaintenanceActor` accepts one global cleanup hint.
3. Writable state opening idempotently ensures an index on
   `last_seen_unix_seconds` without changing the schema version.
4. `AsyncSqliteStateStore` executes one indexed delete using the seven-day
   cutoff.

## Proof

- State test: old rows are deleted; cutoff and fresh rows remain.
- Schema test: opening an existing v13 database creates the timestamp index
  without changing `PRAGMA user_version`.
- Runtime/maintenance test: startup requests cleanup, repeated same-day
  connections do not, and a later UTC day may request it again.
- Failure-isolation test: a degraded attempt is not retried repeatedly on the
  same UTC day and does not affect request routing.
- Focused state/proxy tests, workspace tests, formatting, clippy, and an
  isolated SQLite manual proof must pass.
