# Host state retention audit (S6)

Audit date: 2026-09-17. This is an audit only; no schema or runtime change is proposed.

## Scope and measurement boundary

The production state root (`~/.codex-router`) was not opened or measured because this
assignment runs inside the production Host and the governing safety boundary forbids
access to that root. No isolated debug `state.sqlite` was present under the permitted
temporary roots at audit time, so there are no honest live row-count or byte-size
measurements to report. The table sizes below are therefore cardinality risks derived
from current schemas and write/prune paths, not production measurements.

## Findings

| Table | Growth shape | Existing pruning | Audit result |
| --- | --- | --- | --- |
| `active_session_events` | Append-only acquired/released/retired/stale-purged event rows; schema is an integer-keyed history table ([definitions.rs:113-143](../../crates/codex-router-state/src/account_schema/definitions.rs:113)). | `compact_completed_active_session_events_before(route_band, cutoff)` removes terminal rows and their matching acquired rows only when the whole session is complete ([sqlite.rs:1919-1940](../../crates/codex-router-state/src/sqlite.rs:1919)). | Potentially unbounded if compaction is not called, or while sessions remain open; no automatic retention loop found in `codex-router-state`. |
| `active_session_rollups` | One row per account/route/time bucket ([definitions.rs:144-165](../../crates/codex-router-state/src/account_schema/definitions.rs:144)). | `purge_active_session_rollups_before(cutoff)` deletes buckets ending before the cutoff ([sqlite.rs:2152-2164](../../crates/codex-router-state/src/sqlite.rs:2152)). | Bounded only by callers supplying a cutoff; no table-level automatic policy found. |
| `quota_history_observations` | Autoincrement append-only observations ([definitions.rs:75-90](../../crates/codex-router-state/src/account_schema/definitions.rs:75)). | `purge_quota_history_before(timestamp)` deletes older observations ([sqlite.rs:1767-1781](../../crates/codex-router-state/src/sqlite.rs:1767)). | Potentially unbounded absent scheduled callers; retention interval is not encoded in schema. |
| `previous_response_affinity_owners` | Composite-key rows include `created_unix_seconds`; inserts can create new affinity/account/route combinations ([definitions.rs:67-74](../../crates/codex-router-state/src/account_schema/definitions.rs:67)). | No timestamp-based delete or purge method exists in the state repository; test cleanup uses whole-table deletion only. | Highest clear unbounded-growth risk; requires an owner decision before adding retention. |
| `session_account_affinities` | One row per session, updated by session id; `last_seen_unix_seconds` is retained ([definitions.rs:170-174](../../crates/codex-router-state/src/account_schema/definitions.rs:170)). | `purge_session_account_affinities_before(cutoff)` removes old last-seen rows ([sqlite.rs:1310-1323](../../crates/codex-router-state/src/sqlite.rs:1310)). | Has an explicit purge seam; automatic invocation/interval was not found in this audit. |

For comparison, `lifecycle-observation` already has an explicit rolling 30-day
checkpoint-and-delete implementation: it advances the checkpoint, preserves thread
addresses, deletes expired `lifecycle_records`, and updates payload accounting in one
transaction ([journal_retention.rs:6-89](../../crates/lifecycle-observation/src/journal_retention.rs:6)).

## Recommendation to orchestrator

Do not change schema in S6. Before designing retention, collect production-safe
measurements from an owner-authorized debug/export path for row counts, SQLite page
counts, and file bytes per table, then identify actual callers of the four existing
purge seams. `previous_response_affinity_owners` should be the first design candidate
because it has a creation timestamp but no repository purge operation.
