# sqlx-turso

A SQLx driver for the [Turso](https://github.com/tursodatabase/turso) database engine: one owned
connection per store, compile-time checked queries, SQLx migrations, and Turso Sync push and
pull on the same connection. Router's project stores use it.

## Origin and license

This driver is derived from [`avencera/sqlx-turso`](https://github.com/avencera/sqlx-turso) by
Praveen Perera, licensed **MIT OR Apache-2.0** as declared in its manifests (upstream ships no
separate license file). [`agent-collaboration/sqlx-turso`](https://github.com/agent-collaboration/sqlx-turso)
mirrors upstream. The crates were imported from that mirror at commit `1833f652` (upstream
`b7e5fab9` plus the Turso 0.8.1 pin and its batch-error mapping) and then trimmed and
restructured in this repository. They are distributed under the same terms as the rest of the
repository: MIT OR Apache-2.0.

## Crates

| Crate | Job |
|---|---|
| `sqlx-turso` | What applications import: the driver types and the checked query macros |
| `sqlx-turso-core` | The SQLx `Database` implementation: options, engine connection, executor, values, transactions, migrations |
| `sqlx-turso-macros` | `query!`, `query_as!`, `query_scalar!`, `query_file!`, `query_file_as!`, `query_file_scalar!` |

## Features

| Feature | Default | Enables |
|---|---|---|
| `runtime-tokio` | yes | The Tokio runtime integration |
| `macros` | yes | The checked query macros |
| `migrate` | yes | SQLx's `Migrate` for `TursoConnection` |
| `chrono` | yes | Chrono date and time codecs (UTC datetimes as RFC 3339 text) |
| `sync` | **no** | Synced connections and `sync_push`, `sync_pull`, `sync_checkpoint`, `sync_stats` |

`sync` is opt-in because of TLS providers; see [Sync and rustls](#sync-and-rustls).

## Use

```rust,ignore
use sqlx_turso::{TursoConnectOptions, TursoSyncOptions, sqlx::{ConnectOptions, Connection}};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

let mut store = TursoConnectOptions::new()
    .filename("project.db")
    .create_if_missing(true)
    .with_sync_options(TursoSyncOptions::new("https://hub.example/db/project-17"))
    .connect()
    .await?;

// Migrations own the schema and run in one owned write transaction.
let mut transaction = store.begin_with("BEGIN IMMEDIATE").await?;
MIGRATOR.run_direct(None, &mut *transaction, false).await?;
transaction.commit().await?;

let task = sqlx_turso::query!(r#"SELECT status AS "status!: String" FROM tasks WHERE id = ?"#, id)
    .fetch_one(&mut store)
    .await?;

store.sync_push().await?; // bound it with a timeout; a timed-out push may still have applied
```

- Applications depend on `sqlx` (for `migrate!` and the SQLx traits) without its `sqlite`
  feature, and use `sqlx_turso::query!`, not `sqlx::query!`.
- Foreign keys are on by default. A migration that rebuilds a table other tables reference
  turns them off before the owned transaction (`PRAGMA foreign_keys = OFF`), reinserts rows,
  checks `PRAGMA foreign_key_check` before commit, then turns them back on and reads the flag
  back.
- Synced stores contain Turso's own tables (`turso_cdc`, `turso_sync_last_change_id`, ...) in
  `sqlite_schema`; a schema fingerprint must exclude names starting with `turso_` and
  `__turso_internal`.

## Offline metadata

Checked queries read `query-*.json` files when `SQLX_OFFLINE=true`, which `.cargo/config.toml`
sets for every build. Turso metadata lives beside the crate that invokes the macros
(`crates/<crate>/.sqlx/`), apart from the stock SQLite cache in the root `.sqlx/`.

```sh
python3 scripts/tooling/prepare-sqlx-turso.py          # regenerate
python3 scripts/tooling/prepare-sqlx-turso.py --check  # CI: exact names and bytes
```

The script builds a fresh native Turso schema from the crate's migrations with the
`seed_native_schema` example, then describes every checked query against it, one compiler at a
time because Turso locks the schema file per process. A crate that uses these macros is added to
`TURSO_PREPARATION_TARGETS` and never to the stock `prepare-sqlx.py` targets.

## Tests

The Sync tests run against the `tursodb` 0.8.1 release binary, verified by SHA-256:

```sh
python3 scripts/tooling/install-turso-sync-server.py
cargo test -p sqlx-turso -p sqlx-turso-core -p sqlx-turso-macros --all-features
```

A missing binary fails the Sync tests with the install command. `SQLX_TURSO_SYNC_SERVER` points
them at another `tursodb` 0.8.1.

## Known limitations

| Limitation | Evidence |
|---|---|
| Bind arity is not checked: an extra bind fails only at execution; a missing bind reads NULL | Tested |
| `PRAGMA defer_foreign_keys = ON` does not defer; write parents first | Tested |
| Rebuilding a referenced table with foreign keys on fails; use the policy above | Tested |
| On a synced store, a transaction that writes rows into a table it drops or renames before commit cannot be pushed, so a SQL-only copy-and-rename rebuild fails to replicate; the failed push is not atomic on the hub and readers can pull a half-applied migration. Rebuild by drop, recreate and reinserting rows from the application | Tested |
| No read-only opens; `?mode=ro` is rejected | Tested |
| No `VACUUM`: the engine keeps it behind an experimental flag this driver does not set | Source |
| Each statement step does its file IO (`pread`, `pwrite`, `fsync`) synchronously inside `poll`, on the runtime worker that polls it; opening a store does too | Source |
| A statement waiting on a lock busy-polls its task until `busy_timeout` (about 0.5 s CPU per 2 s wait, measured) | Measured |
| Each synced handle runs one `turso-sync-io` thread with its own small runtime | Source and probes |
| Sync replicates the whole database; there is no table selector | Tested |

## Sync and rustls

Turso's Sync takes `hyper-rustls` with its default features, which turn on rustls's
`aws-lc-rs` provider. Crates that use rustls's `ring` provider in the same build leave rustls
with two providers, and rustls can no longer choose one by itself:

- Sync does not return an error. The `turso-sync-io` worker thread panics while building its
  HTTPS client, and every synced open, push and pull on that handle then hangs until the
  caller's own timeout. Bound them all.
- Any other `ClientConfig::builder()` that relies on the automatic choice panics.
- `cargo build`, `clippy` or `test --workspace --all-features` fails to compile
  `collaboration-service`, because aws-lc-rs adds `From<()>` impls that break its type
  inference.

A process that links Sync together with other rustls clients must install a process default
(`CryptoProvider::install_default`) before its first TLS client or synced open. The Sync tests
install aws-lc-rs for this reason. How Router resolves this is an owner decision for spec 2.
