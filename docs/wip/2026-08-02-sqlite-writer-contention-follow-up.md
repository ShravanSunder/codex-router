# SQLite Writer Contention Follow-up

Status: deferred; no implementation change currently justified
Last verified: 2026-08-02

## Decision

Keep the current SQLite behavior until normal operation demonstrates writer
contention or the product adds another independent write path.

The router-owned database already uses WAL. The weekly quota-floor mutation
already retries its complete transaction on `SQLITE_BUSY` and `SQLITE_LOCKED`
with a bounded deadline. The current evidence shows a working contention path,
not a correctness defect that requires immediate remediation.

Do not introduce an app-wide transaction wrapper, `BEGIN IMMEDIATE` policy, or
jittered retry abstraction preemptively.

## Verified Current State

### WAL

`AsyncSqliteStateStore::open` configures the writable SQLx connection with
`SqliteJournalMode::Wal` and a one-connection pool:

- `crates/codex-router-state/src/sqlite.rs:708`
- `crates/codex-router-state/src/lib.rs:1518` verifies the persisted journal
  mode is `wal`.
- A read-only `PRAGMA journal_mode` against the live router database reported
  `wal` on 2026-08-02.

WAL is the right default for router-owned local state: the router can write
while read-only CLI and status paths continue reading committed snapshots.

### Weekly-floor mutation

`AsyncWeeklyQuotaFloorMutationStore` is a narrow writer for one policy change:

- It opens the existing database with `busy_timeout(0)` and one connection at
  `crates/codex-router-state/src/sqlite.rs:2682`.
- It retries the complete mutation transaction after SQLite busy/locked errors
  at `crates/codex-router-state/src/sqlite.rs:2760`.
- Its fixed retry delays are 25 ms, 50 ms, and 100 ms within a 250 ms deadline
  at `crates/codex-router-state/src/sqlite.rs:59`.
- Each attempt performs only account lookup, policy insert/delete, and commit
  at `crates/codex-router-state/src/sqlite.rs:2813`. No network, provider,
  filesystem traversal, or long computation occurs inside the transaction.
- Busy recognition covers SQLite result codes 5 and 6 plus the corresponding
  locked messages at `crates/codex-router-state/src/sqlite.rs:4843`.

The transaction currently begins through SQLx `pool.begin()`. For SQLite this
is a normal deferred `BEGIN`, not `BEGIN IMMEDIATE`. The only explicit
`BEGIN IMMEDIATE` statements in the state crate are test locks used to simulate
a competing writer.

### Existing proof

These focused tests passed from the repository root on 2026-08-02:

```text
cargo test -p codex-router-state async_writable_store_enables_wal_journal_mode
1 passed; 0 failed; exit 0

cargo test -p codex-router-state weekly_floor_mutation_busy_retry_is_bounded_and_preserves_value
1 passed; 0 failed; exit 0
```

Additional coverage in
`weekly_floor_mutation_commits_after_held_writer_releases` proves that a
mutation succeeds after the competing writer releases its lock.

## Reopen Triggers

Reopen this decision if any of the following becomes true:

1. Normal CLI or interactive use produces `database busy; retry save`.
2. Another process or command gains an independent router-state write path.
3. A cross-process mutation adds a read-modify-write decision whose snapshot
   can become stale before it acquires the writer slot.
4. Runtime evidence shows multiple writers retrying on the same fixed schedule.

Do not reopen it solely because `BEGIN IMMEDIATE` and jitter are generally useful
SQLite techniques. The trigger must come from this application's writer model
or observed contention.

## Smallest Candidate Improvement

If triggers 1-3 occur, change only the weekly-floor mutation first:

```text
BEGIN IMMEDIATE
  -> reread current account state
  -> insert or delete the policy
  -> commit
```

Keep the existing complete-transaction retry boundary, fixed delays, terminal
busy error, and 250 ms deadline for the first slice. This moves contention to
the transaction boundary without introducing randomized timing or a general
retry abstraction.

Add jitter only if trigger 4 is observed. At that point, retain the same bounded
deadline and retry the whole transaction; never retry only the failed statement
against an earlier read snapshot.

## Boundaries

- Keep WAL for router-owned local `state.sqlite`.
- Do not change how read-only Codex session state is opened.
- Do not apply `BEGIN IMMEDIATE` to read-only or may-not-write transactions.
- Do not put network, provider, model, filesystem, or sleep work inside a write
  transaction.
- Do not broaden the first change to every state repository method.
- Do not weaken or remove the terminal database-busy result.

## Proof Required If Reopened

Before implementation, recheck the current SQLx version and its supported API
for starting an immediate SQLite transaction. Then require:

1. A RED test that distinguishes deferred acquisition from immediate writer
   acquisition at the transaction boundary.
2. Existing held-writer tests remain green and preserve the committed value on
   exhausted retries.
3. A held writer that releases within the retry budget still allows the
   mutation to commit after rereading current state.
4. Read-only access remains responsive while a WAL writer is held.
5. State-crate formatting, linting, and tests pass.
6. Manual CLI proof demonstrates both successful save and bounded busy output.

## References

- [SQLite WAL documentation](https://www.sqlite.org/wal.html)
- [SQLite transaction documentation](https://www.sqlite.org/lang_transaction.html)
- `crates/codex-router-state/src/sqlite.rs`
- `crates/codex-router-state/src/lib.rs`
