# sqlx-turso fork in the monorepo: program design

Date: 2026-10-07. Status: draft for review. Requirements: [requirements.md](requirements.md).
Specification: [spec.md](spec.md).

## 1. Crates and dependency direction

```text
application crate ──► sqlx-turso (facade) ──► sqlx-turso-core (driver) ──► turso =0.8.1
        │                     │                        └──► sqlx-core =0.9.0
        │                     └─(feature macros)─► sqlx-turso-macros (proc-macro)
        │                                                └──► sqlx-turso-core [macros], sqlx-macros-core =0.9.0
        └──► sqlx =0.9.0 (generic: Connection/Executor traits, migrate!)   no sqlite feature
```

- `sqlx-turso-core` owns the SQLx `Database` implementation: options, engine connection,
  executor, values, transactions, migrations, macro type checking. Reason to change: Turso or
  SQLx internals change.
- `sqlx-turso-macros` owns the checked-query proc macros. It registers the Turso driver with
  sqlx-macros-core's expander and rewrites generated `::sqlx` paths through the facade. Reason to
  change: the macro surface or SQLx's expander changes.
- `sqlx-turso` is the application import surface: re-exports and the macros. Reason to change:
  the public surface changes. It also hosts the crate README, the Turso offline metadata for its
  tests, the native schema seed tool and the integration tests.
- Three crates stay three: the proc-macro crate must be separate, and the macro needs the
  `DatabaseExt` impl, which orphan rules place in core.

## 2. Module layout

Line counts are targets after trimming; every file stays at or under about 600.

### `crates/sqlx-turso-core/src/`

| File | Responsibility | From upstream |
|---|---|---|
| `lib.rs` | Crate docs, module tree, public re-exports | `lib.rs` |
| `database.rs` | `Turso` marker and `Database` impl | unchanged |
| `connect_options/mod.rs` | `TursoConnectOptions`: fields, getters, builders, `ConnectOptions` impl | `options.rs` 110–563 |
| `connect_options/database_target.rs` | `TursoDatabaseTarget { File, Memory }` and `OpenMode { ReadWrite, CreateIfMissing }` | `options.rs` 25–35, 132–183 |
| `connect_options/sync_options.rs` | `TursoSyncOptions` | `options.rs` 44–52, 791–860 |
| `connect_options/connection_url.rs` | `turso:` URL parsing (`FromStr`) and `to_url_lossy` rendering | `options.rs` 514–529, 565–789 |
| `engine_connection.rs` | Open the local or synced `turso::Database`, connect, apply `busy_timeout` and `foreign_keys` | `driver.rs` (renamed: it opens the engine connection) |
| `connection.rs` | `TursoConnection`, `Connection` impl, transaction-depth state, statement cache, Sync operations | `connection.rs` |
| `executor/mod.rs` | `Executor` impl: fetch, prepare, describe | `executor.rs` 1–116, 192–265 |
| `executor/row_stream.rs` | The row stream state machine over `turso::Rows` | `executor.rs` 118–190 |
| `executor/sql_inspection.rs` | Multi-statement and named-placeholder inspection | `executor.rs` 267–409 |
| `executor/tests.rs` | Executor behaviour tests | `executor.rs` 423–866, trimmed |
| `transaction.rs` | `TursoTransactionManager` | unchanged, MVCC tests removed |
| `migrate.rs` | `Migrate` impl and migration-table identifier validation | `migrate.rs`, lifecycle removed |
| `value/mod.rs` | `TursoValue`, `TursoValueRef`, storage-class access | `value.rs` 1–208 |
| `value/primitive_codecs.rs` | Integer, bool, float, text, blob and `Option` codecs | `value.rs` 210–476, 1032 |
| `value/chrono_codecs.rs` | Chrono codecs and SQLite datetime parsing | `value.rs` 612–845 |
| `macro_type_checking.rs` | `TypeChecking` and `DatabaseExt` for the checked macros | `macros.rs` (renamed: `macros` collides with the feature and macro crate) |
| `error.rs` | `TursoAdapterError` (thiserror enum), `TursoDatabaseError`, error-code and kind mapping | `error.rs` |
| `arguments.rs`, `column.rs`, `row.rs`, `statement.rs`, `type_info.rs`, `query_result.rs` | unchanged responsibilities | unchanged, `any`-only helpers removed |

Deleted files: `any.rs`, `pool.rs`, `lifecycle.rs`, `features.rs` (its surviving Sync local test
moves to `connection.rs` tests), `options.rs`, `value.rs`, `driver.rs`, `macros.rs`.

`TursoAdapterError` becomes a thiserror enum (rubric ERR-01, ERR-03) with one variant per
condition the driver raises: URL (`MissingTursoScheme`, `InvalidUrlPath`,
`UnknownUrlParameter`, `InvalidUrlParameterValue`), unsupported or misused surfaces
(`ReadOnlyUnsupported`, `SyncFeatureDisabled`, `NotSyncConnection`,
`BatchArgumentsUnsupported`, `NamedPlaceholderUnsupported`, `InvalidMigrationTableName`) and decoding
(`StorageClassMismatch { expected, actual: TursoStorageClass }`, `InvalidTemporalText`,
`TemporalOutOfRange`). Configuration and usage variants surface as
`sqlx::Error::Configuration`; decode variants travel SQLx's decode path. Callers downcast the
boxed source to match a variant.

`to_url_lossy` keeps the one production `expect`: `ConnectOptions::to_url_lossy` cannot fail,
every `Url` constructor is fallible, so the constant `turso:` literal is parsed with
`#[expect(clippy::expect_used, reason = …)]`. The alternative, leaving SQLx's default
`unimplemented!()`, would be a reachable panic.

### `crates/sqlx-turso-macros/src/lib.rs`

One file (about 230 lines). The `expect("peeked token must exist")` becomes a match.

### `crates/sqlx-turso/`

```text
README.md                         attribution, license, use, limitations, metadata prep
.sqlx/query-*.json                Turso offline metadata for this crate's checked queries
src/lib.rs                        re-exports
examples/seed_native_schema.rs    applies migration directories to a fresh native database
tests/migrations/1_project_store_slice.sql      checked-query schema (from the Router-path probe)
tests/migration_steps/2_tasks_label.sql         add-column step
tests/migration_steps/3_tasks_rebuild.sql       same-name rebuild step
tests/support/mod.rs              shared test helpers
tests/support/scratch_store.rs    per-test temp directory and connection helpers
tests/support/sync_server.rs      tursodb process harness
tests/support/project_admission.rs  checked-query admission and snapshot over the project slice
tests/checked_query_macros.rs
tests/checked_query_diagnostics.rs   + tests/checked_query_diagnostics/{pass,fail}/*.rs
tests/owned_migration_transactions.rs
tests/foreign_key_enforcement.rs
tests/sync_replication.rs
tests/turso_only_dependency_graph.rs
```

Upstream examples are not imported; the README and rustdoc carry usage.

## 3. Options after the trim

```rust
pub struct TursoConnectOptions(Arc<ConnectOptionsInner>);
struct ConnectOptionsInner {
    target: TursoDatabaseTarget,        // File(PathBuf) | Memory
    open_mode: OpenMode,                // ReadWrite | CreateIfMissing
    busy_timeout: Duration,             // default 5 s
    statement_cache_capacity: usize,    // default 100
    foreign_keys: bool,                 // default true
    sync: Option<TursoSyncOptions>,
    log_settings: LogSettings,
}
```

- URL: `turso:<path>`, `turso://<path>`, `turso::memory:`; query parameter `mode` accepts `rw`,
  `rwc`, `memory`; `ro` returns `UnsupportedSurface("read-only Turso connections")`; any other
  parameter is rejected. `to_url_lossy` renders path and mode only.
- File-existence validation uses `tokio::fs::try_exists`, not `Path::exists`, on the connect path.
- Without the `sync` feature, a connection with Sync options fails with
  `UnsupportedSurface("Turso sync connections require the sync feature")`.

## 4. Features

| Crate | Features | Default |
|---|---|---|
| `sqlx-turso` | `runtime-tokio`, `macros`, `sync`, `migrate`, `chrono` | all but `sync` |
| `sqlx-turso-core` | the same five, forwarded; `macros` also turns on serde for describe metadata | `runtime-tokio` |
| `sqlx-turso-macros` | `chrono` (type mapping) | none |

`sync` is opt-in; the other four are on by default. The first cut defaulted all five so that
workspace lint and test covered every path. Implementation found that this breaks unrelated
crates (§12): Turso's Sync enables rustls's `aws-lc-rs` provider, Router's crates enable `ring`,
and a build with both cannot pick a default provider. With `sync` opt-in, no current workspace
build links Sync, and the Sync paths are covered by their own invocations instead: CI lints the
three driver crates with `--all-features` and runs their tests once, with `--all-features`,
excluding them from the workspace test run. The upstream `offline` feature folds into
`macros`. The reduced build
`cargo check --locked -p sqlx-turso --lib --no-default-features --features runtime-tokio` runs
in CI so the `not(feature = …)` branches keep compiling; it proves that one reduced
configuration, not every combination.

## 5. Workspace integration

- Members: add `crates/sqlx-turso`, `crates/sqlx-turso-core`, `crates/sqlx-turso-macros`.
  Package fields inherit `version`, `edition`, `license`, `publish`. `[lints] workspace = true`.
- `[workspace.dependencies]` additions: `turso = { version = "=0.8.1", default-features = false }`
  (drops `mimalloc` and `fts`), `sqlx-core` and `sqlx-macros-core` at `=0.9.0` without default
  features, `either`, `futures-core`, `log`, `percent-encoding`, `url`, `proc-macro2`,
  `proc-macro-crate`, `quote`, `syn`, `trybuild`, and path entries for the three crates.
- The existing `sqlx` entry becomes engine-agnostic and exact:
  `sqlx = { version = "=0.9.0", default-features = false, features = ["runtime-tokio"] }`. Every
  existing member that inherits it adds `features = ["sqlite"]` (plus the features it already
  lists), so each crate's resolved features are unchanged. Reasons: backend features belong
  with the crates that use them; the driver implements `sqlx-core` internals, so any SQLx bump
  must move in lockstep, which `=` enforces at the workspace level. The guarantee is scoped:
  Cargo unifies features across a combined workspace build, so only a build of the Turso
  packages by themselves is SQLite-free. `tests/turso_only_dependency_graph.rs` asserts that
  with `cargo tree -p sqlx-turso` (no `libsqlite3-sys`, `sqlx-sqlite` or `libsql*`).
- `tempfile` is a dev-dependency declared the way the other crates declare it.
- License: MIT OR Apache-2.0, identical to the workspace. The facade README records the upstream
  project, author, license and the imported commits (`b7e5fab9` upstream, `1833f652` port), and
  points at the mirror. No upstream LICENSE file exists to copy.
- No workspace version bump: no binary changes behaviour (`AGENTS.md` release rule applies to
  user-facing changes).

## 6. Native Turso offline metadata

```text
prepare-sqlx-turso.py [--check]
  for target in TURSO_PREPARATION_TARGETS            # ("sqlx-turso", ("crates/sqlx-turso/tests/migrations",), "crates/sqlx-turso/.sqlx")
    1. scratch dir under tmp/ (TemporaryDirectory)
    2. cargo run --locked -p sqlx-turso --example seed_native_schema -- <scratch>/schema.db <migration dirs…>
    3. cargo clean --locked -p <package>             # proc macros re-run only when the crate recompiles (same reason as verify-sqlx-contract.py)
    4. cargo check --locked -j 1 -p <package> --all-targets --all-features
         env: SQLX_OFFLINE=false  DATABASE_URL=turso:<scratch>/schema.db  SQLX_OFFLINE_DIR=<scratch>/metadata
    5. --check: committed set == staged set (names and bytes) else exit 1 with the differing names
       write:   replace <package>/.sqlx/query-*.json with the staged set
```

- `-j 1`: Turso holds a per-process lock on the schema file; parallel rustc processes describing
  against one file fail with `File is locked by another process` (Router-path proof §Commands).
  Dependencies are already built by the preceding clippy step in CI, so only the package's own
  targets compile serially.
- `cargo clean -p` is chosen over touching source mtimes (what sqlx-cli does): it forces
  re-expansion deterministically and changes no tracked file; it costs a rebuild of the
  package's own targets.
- The seed tool uses no checked macro, so it compiles before any metadata exists. It opens the
  fresh file with `create_if_missing`, refuses an existing file, merges
  the given migration directories (versions must be unique), runs them with `run_direct` inside
  `BEGIN IMMEDIATE`, checks `PRAGMA foreign_key_check`, and commits.
- Isolation from the stock check: different directory (`crates/sqlx-turso/.sqlx` versus root
  `.sqlx`); the stock script only compiles its four packages, so it never expands a Turso macro;
  the stock write mode deletes only root `query-*.json`. The macro lookup order (offline dir,
  crate `.sqlx`, workspace `.sqlx`) finds Turso metadata first for the Turso crate. The
  argument depends on one rule (spec S5): a crate that invokes Turso macros is a Turso target,
  never a stock target, because the stock script's `SQLX_OFFLINE_DIR` would otherwise win.
- The script has unit tests in `scripts/tests/test_prepare_sqlx_turso.py` with a fake process
  runner, mirroring `test_prepare_sqlx.py`.
- CI `lint` job, after the stock check: the unit test, `prepare-sqlx-turso.py --check`, and the
  reduced-feature `cargo check`.

## 7. Sync server tooling and harness

- `scripts/tooling/install-turso-sync-server.py [--check]` downloads
  `turso_cli-<target>.tar.xz` for v0.8.1 from the GitHub release, verifies the SHA-256 pinned in
  the script for `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-unknown-linux-gnu` and
  `x86_64-unknown-linux-gnu`, extracts only `tursodb` into `tmp/rust-tools/bin/`, and verifies
  `tursodb --version` reports `Turso 0.8.1`. `--check` verifies without downloading. Unit tests
  cover target selection, digest mismatch and the version check.
- CI `test` job runs the installer (cached on the script's hash), then runs the three driver
  crates' tests with `--all-features` in their own `nextest` invocation, excluded from the
  workspace run. The trybuild test gets a longer `slow-timeout` in `.config/nextest.toml`.
- `tests/support/sync_server.rs`: `SyncServer::start_single_file` and
  `SyncServer::start_directory`; checks `tursodb --version` reports `Turso 0.8.1` (also for an
  override binary), binds a free `127.0.0.1` port, spawns `tursodb`, waits for TCP readiness
  under a 15 s `tokio::time::timeout`, and exposes `restart` and an awaited `shutdown` that kills
  and reaps under a timeout. `kill_on_drop` is only the fallback for a panicking test. Binary
  lookup: `SQLX_TURSO_SYNC_SERVER`, else `<workspace>/tmp/rust-tools/bin/tursodb`; absent →
  error naming the installer.
- Every push, pull and connect in tests is wrapped in a bounded `timeout`.

## 8. Test plan

Unit tests stay beside the code they test (ported upstream tests minus deleted surfaces).

| Area | Kept upstream tests | Added |
|---|---|---|
| Options and URL | memory/file parsing, defaults, rejection of unknown params, `ConnectOptions` URL hooks, missing-file `NotFound`, create-if-missing | `mode=ro` explicit rejection; `foreign_keys(false)` reads back 0 |
| Executor | literal SQL, batches, optional fetch, quoted `;`, positional binds, named-placeholder rejection, storage classes, affinity, describe metadata, name lookup, error mapping, offline serialization, chrono round trip, dropped streams, statement cache | — |
| SQL inspection | (new focused unit tests) | comments, quoted strings, trailing `;` |
| Transactions | commit, rollback, savepoints, custom begin, drop rollback, failed-rollback poisoning | — |
| Migrations | apply/list/revert, quoted table names, dirty record, invalid identifiers | — |
| Values | Julian-day decode | — |
| Errors | — | `BatchStatementFailed` and `BatchRollbackFailed` code mapping |
| Sync (no server) | local execution without bootstrap, stats, checkpoint | `sync_push` on a non-sync connection is `UnsupportedSurface` |

Integration tests in `crates/sqlx-turso/tests/` (permanent forms of the probes):

| Test file | Proves | Probe source |
|---|---|---|
| `checked_query_macros.rs` | `query!`, `query_as!`, `query_scalar!`, `query_file*!` against the migrated native schema, offline metadata; **negative**: extra bind fails at execution, missing bind reads NULL | `driver_probe`, macro tests |
| `checked_query_diagnostics.rs` | trybuild: pass cases; **fail**: unknown column (`no such column`), scalar with two columns. Re-executes its own test binary for exactly this test (`--exact`), with a recursion-guard variable, `DATABASE_URL=turso::memory:`, `SQLX_OFFLINE=false` and a private `SQLX_OFFLINE_DIR`, under a bounded wait, and fails with the child's status — so no `unsafe set_var` | upstream `ui.rs`, `driver_negative_query` |
| `owned_migration_transactions.rs` | `run_direct` in owned `BEGIN IMMEDIATE`: rollback leaves no user tables and no `_sqlx_migrations`, rerun commits; add-column step matches a fresh native fingerprint; same-name rebuild under the FK-off policy keeps the row graph, `foreign_key_check` empty, FKs read back 1; **negative**: the same rebuild with FKs on fails with a foreign-key violation even with `defer_foreign_keys=ON` | `migration_probe`, D5 |
| `foreign_key_enforcement.rs` | FKs on by default and read back; missing parent rejected as `ForeignKeyViolation`; parent-before-child in one IMMEDIATE transaction commits; **negative**: child-before-parent with `defer_foreign_keys=ON` fails immediately | D2 |
| `sync_replication.rs` | writer pushes, persistent reader stale before pull and current after, with checked queries and the same connection; an update reaches the reader's cached statement; **atomic admissions**: writer admissions (task state, event, ledger decision in one IMMEDIATE transaction) interleave with reader pulls, and every reader snapshot — read in one transaction — has exact task-state, event-payload, revision and ledger agreement, dense positions and an empty FK check, ending equal to the writer's; hub stopped: push fails within the bound, local writes continue, restart and push backlog, reader exact; add-column **and FK-off same-name rebuild** migrations pushed and pulled, reader fingerprint and row graph equal the writer's; directory-mode hub serves two databases independently | `driver_probe`, D1, D4, D5, outage, many-databases Q1 |
| `turso_only_dependency_graph.rs` | `cargo tree -p sqlx-turso` (normal and dev edges) contains no `libsqlite3-sys`, `sqlx-sqlite` or `libsql*` package | trim-and-port §1 |

Not carried as permanent tests, with reason: the gateway 413 body limit (gateway property, spec
2); lease fencing and stale-writer refusal (gateway, spec 2); descriptor-exhaustion worker panic
(needs a process-wide rlimit change); thread inventories (needs unsafe Mach calls); cancellation
barriers (need the PoC gateway).

## 9. Implementation sequence (local commits)

1. Design and trace.
2. Import the three crates verbatim from the 0.8.1 port commit, wire members and workspace
   dependencies, adjust the workspace `sqlx` entry. Builds; lints not yet clean.
3. Trim: delete the removed surfaces and their tests; fold `offline` into `macros`.
4. Split `options.rs`, `value.rs`, `executor.rs`; rename `driver.rs`, `macros.rs`; thiserror
   adapter error; async file-existence check; lint-clean.
5. Facade: README, seed example, integration tests, migrations, diagnostics, support harness.
6. Tooling: `prepare-sqlx-turso.py`, `install-turso-sync-server.py`, their unit tests, generated
   `.sqlx`, CI wiring.
7. Proof run and trace checkpoint.

## 10. Tradeoffs named

| Choice | Gain | Cost |
|---|---|---|
| High-level Sync API (decided) | Proven path, small port | One `turso-sync-io` thread per synced handle; inline blocking page IO on runtime workers (raised to the board-design Lead for spec 2: accept the exception, or isolate SQL execution from executor workers); no bind arity |
| `sync` opt-in, other features default | No workspace build links Sync, so Router's TLS clients keep one rustls provider | Sync lint and tests need their own `--all-features` invocations; the consumer that enables `sync` inherits the provider choice (§12) |
| `macros` feature links `sqlx-macros-core` into the runtime graph (upstream design) | `DatabaseExt` can live where orphan rules allow | Larger dependency graph for applications that enable `macros`. Narrowing it is follow-up work. |
| Workspace `sqlx` without `sqlite` | Turso crates stay Turso-only; exact SQLx pin visible | Thirteen existing manifests each gain `features = ["sqlite"]` |
| Crate-local `.sqlx` plus a second prep script | Zero interaction with the stock cache and script | Two prep commands; CI runs both |
| Required `tursodb` binary for tests | Sync proof runs in every CI run, against the exact release | A download step in CI and a one-time local install |
</content>
</invoke>

## 11. Advisor critique and disposition

GPT 6.1 Sol xhigh reviewed this design read-only on 2026-10-07.

| # | Advice | Disposition |
|---|---|---|
| 1 | Keep the workspace `sqlx` change, but scope the SQLite-free guarantee to separate Turso builds and check it | Accepted: §5 wording and `turso_only_dependency_graph.rs` |
| 2 | Isolation holds only while no stock target invokes Turso macros; prefer package clean; keep the seed tool metadata-free | Accepted: spec S5 rule, §6 notes |
| 3 | Pinned release installer; also verify an override binary | Accepted: version check in the harness |
| 4 | All-five defaults; reduced check as `--locked --lib --no-default-features --features runtime-tokio` | Reduced check accepted. All-five defaults accepted, then reversed on implementation evidence (§12): `sync` is opt-in |
| 5 | Trims fine; Vacuum removal makes `VACUUM` unavailable even as SQL; local read-only does not need the lower SDK | Accepted: spec S3 wording corrected |
| 6 | Strengthen proof: replicated FK-off rebuild; keep D1's transactional snapshots, exact agreement and interleaving; trybuild re-exec needs guard, exact selection, private metadata, bounded child, status | Accepted: §8 |
| 7 | Raise inline blocking page IO to the board-design Lead as a spec-2 choice | Accepted: open question in the report; spec S4 and §10 |
| 8 | Thiserror enum is in scope; do not discard parse causes | Partly accepted: temporal variants keep the offending text and kind. Rejected attaching chrono's `ParseError`: decoding tries up to twelve formats, so no single parse error is the cause, and the stored text is the diagnostic a caller needs. |
| 9 | S4 overclaimed tests for every limitation; qualify Sync thread termination | Accepted: S4 now marks tested versus source-established limitations |
| — | Bounded awaited server shutdown, kill-on-drop only as fallback; redact the auth token in `Debug` | Accepted: §7 and `TursoSyncOptions` `Debug` |

## 12. Findings during implementation

| Finding | Evidence | Disposition |
|---|---|---|
| Turso's Sync and Router's TLS clients cannot share a build without an explicit rustls provider | turso 0.8.1 takes `hyper-rustls` with default features (`aws-lc-rs`); Router enables `ring`. Unified build: `cargo test -p codex-router-proxy -p sqlx-turso --lib -- claude_edge::upstream_endpoint` → 3 tests panic in `rustls::crypto::CryptoProvider` ("Could not automatically determine the process-level CryptoProvider"); `collaboration-service` stops compiling (E0282/E0283: aws-lc-rs adds `From<()>` impls). Turso's Sync worker calls `with_native_roots()` the same way. | `sync` opt-in (§4); Sync tests install the aws-lc-rs default; open question for spec 2 (which provider a Router binary that links Sync installs, or a Turso patch) |
| Synced stores expose Turso's internal tables in `sqlite_schema` | The replicated-migration test saw `turso_cdc`, `turso_cdc_version`, `turso_sync_last_change_id`, `__turso_internal_seq_…` on the synced reader | Schema fingerprints exclude `sqlite_`, `turso_` and `__turso_internal` names; spec-2 schema validation must do the same |
| Turso 0.8.2 now exists; internal crates float within `^0.8.1` | A fresh resolve picked 0.8.2 for eight of the nine Turso packages | Lockfile pinned to 0.8.1 for all nine with `cargo update --precise`; `--locked` keeps it |
| The pinned release binary is the probes' binary | `tmp/rust-tools/bin/tursodb` SHA-256 `fe4e1435…` equals the investigation's recorded digest | — |
