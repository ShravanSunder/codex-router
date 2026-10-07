# sqlx-turso fork in the monorepo: specification

Date: 2026-10-07. Status: draft for review. Requirements: [requirements.md](requirements.md).
Structure: [program-design.md](program-design.md).

## S1. Entities

| Entity | Meaning |
|---|---|
| `Turso` | The SQLx database marker for the Turso engine. URL scheme `turso:`. |
| `TursoConnectOptions` | How to open one store: a target, an open mode, busy timeout, statement-cache capacity, foreign-key enforcement, optional Sync settings, statement logging. |
| `TursoDatabaseTarget` | `File(path)` or `Memory`. A memory target is private to its one connection. |
| Open mode | `ReadWrite` (file must exist) or `CreateIfMissing`. There is no read-only mode. |
| `TursoSyncOptions` | Remote base URL, optional auth token, client name, long-poll timeout, bootstrap-if-empty. Configured in code only. |
| `TursoConnection` | One owned engine connection. With Sync (opt-in feature) it also holds the Sync database handle that `sync_push`, `sync_pull`, `sync_checkpoint` and `sync_stats` act on. |
| Offline metadata | `query-<hash>.json` files that the checked macros read when `SQLX_OFFLINE=true`. Turso metadata lives beside the crate that invokes the macros. |
| Native schema database | A fresh Turso file built by applying a crate's migrations through this driver. Metadata is described against it, never against SQLite. |
| Sync server | `tursodb` 0.8.1 with `--sync-server`, single-file or `--sync-dir` directory mode. Test infrastructure here; production service is spec 2. |

## S2. Observable behaviour that stays

- **Connect.** `turso:<path>` opens a file that must exist; `?mode=rwc` creates it;
  `turso::memory:` (or `?mode=memory`) opens a private memory database. Each connection applies
  `busy_timeout` and `PRAGMA foreign_keys` (ON by default) before it is returned.
- **Execute and fetch.** Literal and prepared SQL, positional (`?`, `?N`, `$N`) arguments,
  multi-statement batches without arguments, streamed rows, dropped streams leaving the
  connection reusable, a bounded statement cache, SQLite-compatible error codes and SQLx error
  kinds (unique, foreign key, not null, check).
- **Values.** NULL, INTEGER, REAL, TEXT and BLOB storage; `bool`, signed and unsigned integers,
  floats, `String`/`&str`, `Vec<u8>`/`&[u8]`, `Option<T>`; chrono date, time and datetime types,
  with UTC datetimes encoded as RFC 3339 text.
- **Transactions.** `begin`, `begin_with("BEGIN IMMEDIATE")`, nested savepoints, commit,
  rollback, rollback on drop at next use, and a connection marked unusable if that rollback fails.
- **Migrations.** `sqlx::migrate!` and runtime `Migrator` run through `Migrate` for
  `TursoConnection`, including `run_direct` inside a caller-owned `BEGIN IMMEDIATE` transaction,
  where a nested migration transaction becomes a savepoint. Migration table names are validated
  identifiers.
- **Checked macros.** `query!`, `query_as!`, `query_scalar!`, `query_file!`, `query_file_as!`,
  `query_file_scalar!` describe against the native engine online, or read committed metadata
  offline. Parameter checking stays weak (no arity check).
- **Sync.** A synced connection pushes local changes and pulls remote ones on the same handle; a
  persistent reader sees pulled rows through its existing connection and cached statements. The
  whole database replicates; there is no table selector.

## S3. Surfaces removed (hard cutover)

| Removed | Replacement or reason |
|---|---|
| `Any` driver, `install_turso_any_driver` | Stores know their database type. |
| `TursoPool`, `TursoPoolOptions`, `TursoExecutor`, pool tests and example | One owned connection per store. Generic `Pool<Turso>` from SQLx core is not offered or tested. |
| `fts` feature, index-method option | Not used; drops Tantivy and its graph. |
| `time`, `uuid`, `json` value integrations | Store IDs and JSON as TEXT in private rows and decode into domain types. Chrono stays. |
| Shared-cache memory databases, named memory targets | No multi-connection memory stores. |
| Encryption, MVCC, `BEGIN CONCURRENT` tests, attach, custom types, generated columns, materialized views, multiprocess WAL, Vacuum, WITHOUT ROWID options | The experimental options matrix. Consequence: `VACUUM` is unavailable even as SQL, because the engine needs its experimental flag; store upkeep that needs it reopens this. |
| VFS name, custom IO backend, immutable flag | Not used; custom IO also exposed engine internals in the public API. |
| `read_only(bool)` | Read-only opens are deferred by the owner. `?mode=ro` is rejected with an explicit error. A local read-only open is a small later addition on the high-level API; a pullable Sync reader that refuses local writes is a separate capability. |
| Generic `pragma(key, value)` list with SQLCipher placeholders | Typed `foreign_keys(bool)`. Other PRAGMAs run as SQL on the connection when a store needs them. |
| `sync_*` URL query parameters | Sync is configured in code; tokens never travel in URLs or `to_url_lossy`. |
| `MigrateDatabase` create/exists/drop and sidecar deletion | Store startup owns its files. |
| `TursoDescribeExt`, `TursoTypeChecking` marker traits | Empty and unused. |
| `sqlx-turso-cli` | Metadata preparation belongs to repository tooling (S5). |

## S4. Known limitations kept visible

"Tested" limitations have a permanent negative test (program design §8), so a Turso behaviour
change fails a test instead of passing silently. "Source" limitations are established from the
Turso 0.8.1 source and the investigation probes; a permanent test would need process-wide or
unsafe instrumentation.

| Limitation | Observable today | Evidence |
|---|---|---|
| Weak bind arity | Extra binds compile and fail at execution; a missing bind compiles and reads NULL. | Tested |
| `PRAGMA defer_foreign_keys=ON` does not defer | A child row inserted before its parent fails immediately with a foreign-key violation. | Tested |
| Same-name table rebuild with incoming FKs enabled | The migration fails with a foreign-key violation. The FK-off-before-transaction policy succeeds. | Tested |
| No read-only opens | `?mode=ro` is rejected. | Tested |
| Inline blocking page IO | A statement step runs its pending page IO inside `poll` (turso 0.8.1 `Statement::step` → `run_io`); the default Unix backend issues synchronous `pread`, `pwrite` and `fsync` on the calling runtime worker. No bound on its duration is established. Raised for spec 2. | Source |
| One Sync IO thread per synced handle | Each synced open starts one detached `turso-sync-io` thread with its own small runtime. After a drained final push, dropping the handle ended it in the probes; termination while an HTTP request hangs is unverified. | Source and probes |
| Push/pull timeouts are caller-side | A timed-out or dropped push may still have been applied remotely. | Probes |
| Whole-database replication | No table selector exists on this API. | Tested (every replication test reads all tables) |
| Turso's own tables on synced stores | `sqlite_schema` lists `turso_cdc`, `turso_sync_last_change_id` and similar on a synced store; schema checks must exclude them. | Tested |
| Sync needs an explicit rustls provider beside other rustls clients | Sync enables rustls's `aws-lc-rs`; with `ring` also enabled, `ClientConfig::builder()` panics unless a process default is installed. | Reproduced; Sync tests install a default |

## S5. Offline metadata contract

- Turso offline metadata for crate `C` lives in `crates/C/.sqlx/`. The root `.sqlx/` remains the
  stock SQLite cache owned by `scripts/tooling/prepare-sqlx.py`.
- `scripts/tooling/prepare-sqlx-turso.py` regenerates it: build a fresh native schema database
  from `C`'s declared migrations through this driver, describe every checked query of `C` against
  it with a forced online compile, and replace `crates/C/.sqlx/query-*.json` only when every
  step succeeds.
- `--check` fails unless the freshly described set equals the committed set exactly: same file
  names, same bytes. Missing, extra and changed files all fail.
- Ordinary builds stay offline (`SQLX_OFFLINE=true` from `.cargo/config.toml`).
- A crate that invokes Turso macros is registered as a Turso preparation target and never as a
  stock target. The isolation holds only under that rule: the stock script sets
  `SQLX_OFFLINE_DIR`, which macros consult before a crate-local `.sqlx`.

## S6. Sync server contract for tests

- Sync tests run against `tursodb` 0.8.1 from the release artifact whose SHA-256 is pinned in the
  repository. The binary is found at `SQLX_TURSO_SYNC_SERVER` or
  `tmp/rust-tools/bin/tursodb`.
- Whichever binary is used, the harness checks that `tursodb --version` reports `Turso 0.8.1`
  before starting it. A missing or wrong binary fails the test with the install command; tests
  never skip silently.
- Servers bind ephemeral `127.0.0.1` ports, use per-test scratch directories and are stopped and
  reaped by the test that started them through an awaited, bounded shutdown; kill-on-drop is
  only the fallback for a panicking test.
</content>
</invoke>
