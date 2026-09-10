use std::collections::BTreeMap;

use sqlx::Row;
use sqlx::SqliteConnection;
use sqlx::SqlitePool;
use sqlx::migrate::MigrateError;

use crate::account_schema::LeaseShape;
use crate::account_schema::LegacyShape;
use crate::account_schema::validate_legacy_schema;
use crate::account_schema::validate_target_schema;
use crate::sqlite::StateStoreError;

pub(crate) static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

const BASELINE_SQL: &str = include_str!("../migrations/202609100001_account_baseline.sql");
const LEASE_V7_TO_BASELINE_SQL: &str =
    include_str!("../legacy-migrations/lease-v7-to-baseline.sql");
const POLICY_V11_TO_BASELINE_SQL: &str =
    include_str!("../legacy-migrations/policy-v11-to-baseline.sql");

const INCOMPATIBLE_HISTORY_MESSAGE: &str = "incompatible account migration history";
const DIRTY_HISTORY_MESSAGE: &str = "dirty account migration history";
const MIGRATION_CONFIGURATION_MESSAGE: &str = "incompatible account migration configuration";
const WRITABLE_UPGRADE_REQUIRED_MESSAGE: &str = "account database requires a writable migration";
const PRESERVATION_FAILURE_MESSAGE: &str = "account migration changed preserved state";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrationAuthority {
    Legacy,
    NativeCurrent,
    NativeUpgradeRequired,
}

pub(crate) async fn migrate(pool: &SqlitePool) -> Result<(), StateStoreError> {
    let mut transaction = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    migrate_transaction(&mut transaction).await?;
    transaction
        .commit()
        .await
        .map_err(crate::sqlite::sqlx_error)
}

// SQLx provides run_direct to avoid generic Acquire lifetime bounds crossing Send futures.
// The caller already owns this transaction and its concrete SQLite connection.
async fn migrate_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), StateStoreError> {
    if native_history_table_exists(&mut *transaction).await? {
        MIGRATOR
            .run_direct(None, &mut **transaction, false)
            .await
            .map_err(map_migration_error)?;
        validate_target_schema(transaction).await?;
        preserve_legacy_version_marker(transaction).await?;
    } else {
        migrate_legacy_or_fresh_database(transaction).await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn migrate_and_rollback_for_test(
    pool: &SqlitePool,
) -> Result<(), StateStoreError> {
    let mut transaction = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    migrate_transaction(&mut transaction).await?;
    transaction
        .rollback()
        .await
        .map_err(crate::sqlite::sqlx_error)
}

#[cfg(test)]
pub(crate) async fn migrate_with_process_checkpoint_for_test(
    pool: &SqlitePool,
    commit_before_checkpoint: bool,
) -> Result<(), StateStoreError> {
    let mut transaction = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    migrate_transaction(&mut transaction).await?;
    if commit_before_checkpoint {
        transaction
            .commit()
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        process_checkpoint_handshake()?;
        return Ok(());
    }
    process_checkpoint_handshake()?;
    transaction
        .commit()
        .await
        .map_err(crate::sqlite::sqlx_error)
}

#[cfg(test)]
fn process_checkpoint_handshake() -> Result<(), StateStoreError> {
    let checkpoint_path = std::env::var("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_CHECKPOINT")
        .map_err(|_| static_sqlite_error("migration test checkpoint path is missing"))?;
    let release_path = std::env::var("CODEX_ROUTER_ACCOUNT_MIGRATION_TEST_RELEASE")
        .map_err(|_| static_sqlite_error("migration test release path is missing"))?;
    let checkpoint_temporary_path = format!("{checkpoint_path}.tmp");
    std::fs::write(&checkpoint_temporary_path, b"checkpoint\n")
        .map_err(|_| static_sqlite_error("migration test checkpoint failed"))?;
    std::fs::rename(checkpoint_temporary_path, checkpoint_path)
        .map_err(|_| static_sqlite_error("migration test checkpoint failed"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !std::path::Path::new(&release_path).exists() {
        if std::time::Instant::now() >= deadline {
            return Err(static_sqlite_error("migration test release timed out"));
        }
        std::thread::yield_now();
    }
    Ok(())
}

pub(crate) async fn migration_authority(
    connection: &mut SqliteConnection,
) -> Result<MigrationAuthority, StateStoreError> {
    if !native_history_table_exists(&mut *connection).await? {
        return Ok(MigrationAuthority::Legacy);
    }

    let applied_rows =
        sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(connection)
            .await
            .map_err(|_| static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE))?;
    let migrations = MIGRATOR.iter().collect::<Vec<_>>();

    for row in &applied_rows {
        let version = row
            .try_get::<i64, _>(0)
            .map_err(|_| static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE))?;
        let success = row
            .try_get::<bool, _>(1)
            .map_err(|_| static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE))?;
        if !success {
            return Err(static_sqlite_error(DIRTY_HISTORY_MESSAGE));
        }
        let Some(migration) = migrations
            .iter()
            .find(|migration| migration.version == version)
        else {
            return Err(static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE));
        };
        let checksum = row
            .try_get::<Vec<u8>, _>(2)
            .map_err(|_| static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE))?;
        if checksum.as_slice() != migration.checksum.as_ref() {
            return Err(static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE));
        }
    }

    if applied_rows.len() == migrations.len() {
        Ok(MigrationAuthority::NativeCurrent)
    } else {
        Ok(MigrationAuthority::NativeUpgradeRequired)
    }
}

pub(crate) async fn validate_native_read_only_schema(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    match migration_authority(connection).await? {
        MigrationAuthority::NativeCurrent => validate_target_schema(connection).await,
        MigrationAuthority::NativeUpgradeRequired => {
            Err(static_sqlite_error(WRITABLE_UPGRADE_REQUIRED_MESSAGE))
        }
        MigrationAuthority::Legacy => Err(static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE)),
    }
}

async fn migrate_legacy_or_fresh_database(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<(), StateStoreError> {
    let legacy_version = sqlx::query_scalar::<_, i64>("PRAGMA user_version")
        .fetch_one(&mut **transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    let has_router_objects = router_object_count(transaction).await? != 0;
    if legacy_version == 0 && !has_router_objects {
        MIGRATOR
            .run_direct(None, &mut **transaction, false)
            .await
            .map_err(map_migration_error)?;
        validate_target_schema(transaction).await?;
        preserve_legacy_version_marker(transaction).await?;
        return Ok(());
    }

    let legacy_shape = validate_legacy_schema(transaction, legacy_version).await?;
    let before = CriticalStateSnapshot::capture(transaction).await?;
    apply_legacy_conversion(transaction, &legacy_shape).await?;
    sqlx::raw_sql(BASELINE_SQL)
        .execute(&mut **transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    validate_target_schema(transaction).await?;

    let after = CriticalStateSnapshot::capture(transaction).await?;
    if before != after {
        return Err(static_sqlite_error(PRESERVATION_FAILURE_MESSAGE));
    }
    let baseline_version = MIGRATOR
        .iter()
        .next()
        .ok_or_else(|| static_sqlite_error(MIGRATION_CONFIGURATION_MESSAGE))?
        .version;
    MIGRATOR
        .run_direct(Some(baseline_version), &mut **transaction, true)
        .await
        .map_err(map_migration_error)?;
    MIGRATOR
        .run_direct(None, &mut **transaction, false)
        .await
        .map_err(map_migration_error)?;
    preserve_legacy_version_marker(transaction).await
}

async fn apply_legacy_conversion(
    connection: &mut SqliteConnection,
    legacy_shape: &LegacyShape,
) -> Result<(), StateStoreError> {
    if matches!(legacy_shape.lease_shape, LeaseShape::VersionSeven) {
        let duplicate_target_keys: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM (
                SELECT route_band, reservation_id
                  FROM active_client_leases
                 GROUP BY route_band, reservation_id
                HAVING COUNT(*) > 1
             )",
        )
        .fetch_one(&mut *connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
        if duplicate_target_keys != 0 {
            return Err(static_sqlite_error("incompatible account database schema"));
        }
        sqlx::raw_sql(LEASE_V7_TO_BASELINE_SQL)
            .execute(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    }

    for column_name in &legacy_shape.missing_event_columns {
        let statement = match *column_name {
            "logical_session_id" => {
                "ALTER TABLE active_session_events ADD COLUMN logical_session_id TEXT NOT NULL DEFAULT ''"
            }
            "session_started_unix_seconds" => {
                "ALTER TABLE active_session_events ADD COLUMN session_started_unix_seconds INTEGER NOT NULL DEFAULT 0"
            }
            "session_ended_unix_seconds" => {
                "ALTER TABLE active_session_events ADD COLUMN session_ended_unix_seconds INTEGER"
            }
            "transport_kind" => {
                "ALTER TABLE active_session_events ADD COLUMN transport_kind TEXT NOT NULL DEFAULT 'unknown'"
            }
            _ => return Err(static_sqlite_error("incompatible account database schema")),
        };
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    }
    for column_name in &legacy_shape.missing_rollup_columns {
        let statement = match *column_name {
            "completed_sessions" => {
                "ALTER TABLE active_session_rollups ADD COLUMN completed_sessions INTEGER NOT NULL DEFAULT 0"
            }
            "stale_purged_sessions" => {
                "ALTER TABLE active_session_rollups ADD COLUMN stale_purged_sessions INTEGER NOT NULL DEFAULT 0"
            }
            _ => return Err(static_sqlite_error("incompatible account database schema")),
        };
        sqlx::query(statement)
            .execute(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    }
    if legacy_shape.policy_needs_rebuild {
        sqlx::raw_sql(POLICY_V11_TO_BASELINE_SQL)
            .execute(&mut *connection)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
    }
    Ok(())
}

async fn native_history_table_exists(
    connection: &mut SqliteConnection,
) -> Result<bool, StateStoreError> {
    sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = '_sqlx_migrations'
         )",
    )
    .fetch_one(&mut *connection)
    .await
    .map_err(crate::sqlite::sqlx_error)
}

async fn router_object_count(connection: &mut SqliteConnection) -> Result<i64, StateStoreError> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE name IN (
            'accounts', 'quota_snapshots', 'affinity_pins', 'selector_quota_windows',
            'quota_refresh_status', 'previous_response_affinity_owners',
            'quota_history_observations', 'active_client_leases',
            'route_band_account_states', 'active_session_events',
            'active_session_rollups', 'account_routing_policies',
            'session_account_affinities'
         )",
    )
    .fetch_one(connection)
    .await
    .map_err(crate::sqlite::sqlx_error)
}

async fn preserve_legacy_version_marker(
    connection: &mut SqliteConnection,
) -> Result<(), StateStoreError> {
    sqlx::query("PRAGMA user_version = 13")
        .execute(connection)
        .await
        .map(|_| ())
        .map_err(crate::sqlite::sqlx_error)
}

#[derive(Debug, Eq, PartialEq)]
struct CriticalStateSnapshot(BTreeMap<&'static str, Vec<Vec<Option<String>>>>);

impl CriticalStateSnapshot {
    async fn capture(connection: &mut SqliteConnection) -> Result<Self, StateStoreError> {
        let mut tables = BTreeMap::new();
        for (table_name, columns, order_by) in [
            (
                "accounts",
                &[
                    "account_id",
                    "label",
                    "status",
                    "active_credential_generation",
                ][..],
                "account_id",
            ),
            (
                "account_routing_policies",
                &["account_id", "weekly_quota_floor_basis_points"][..],
                "account_id",
            ),
            (
                "affinity_pins",
                &["affinity_key", "account_id"][..],
                "affinity_key",
            ),
            (
                "previous_response_affinity_owners",
                &[
                    "affinity_key_hash",
                    "route_band",
                    "account_id",
                    "credential_generation",
                    "source_transport",
                    "created_unix_seconds",
                ][..],
                "affinity_key_hash, route_band, account_id",
            ),
            (
                "session_account_affinities",
                &["session_id", "account_id", "last_seen_unix_seconds"][..],
                "session_id",
            ),
        ] {
            tables.insert(
                table_name,
                capture_table(connection, table_name, columns, order_by).await?,
            );
        }
        Ok(Self(tables))
    }
}

async fn capture_table(
    connection: &mut SqliteConnection,
    table_name: &'static str,
    columns: &[&str],
    order_by: &'static str,
) -> Result<Vec<Vec<Option<String>>>, StateStoreError> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
    )
    .bind(table_name)
    .fetch_one(&mut *connection)
    .await
    .map_err(crate::sqlite::sqlx_error)?;
    if !exists {
        return Ok(Vec::new());
    }
    let projections = columns
        .iter()
        .map(|column| format!("CAST(\"{column}\" AS TEXT)"))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!("SELECT {projections} FROM \"{table_name}\" ORDER BY {order_by}");
    let rows = sqlx::query(sqlx::AssertSqlSafe(query))
        .fetch_all(connection)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (0..columns.len())
                .map(|index| row.get::<Option<String>, _>(index))
                .collect()
        })
        .collect())
}

#[allow(deprecated)]
fn map_migration_error(error: MigrateError) -> StateStoreError {
    match error {
        MigrateError::Execute(error) | MigrateError::ExecuteMigration(error, _) => {
            crate::sqlite::sqlx_error(error)
        }
        MigrateError::Dirty(_) => static_sqlite_error(DIRTY_HISTORY_MESSAGE),
        MigrateError::VersionMissing(_)
        | MigrateError::VersionMismatch(_)
        | MigrateError::VersionNotPresent(_)
        | MigrateError::VersionTooOld(_, _)
        | MigrateError::VersionTooNew(_, _) => static_sqlite_error(INCOMPATIBLE_HISTORY_MESSAGE),
        MigrateError::Source(_)
        | MigrateError::ForceNotSupported
        | MigrateError::InvalidMixReversibleAndSimple
        | MigrateError::CreateSchemasNotSupported(_)
        | MigrateError::SkipNotSupported() => static_sqlite_error(MIGRATION_CONFIGURATION_MESSAGE),
        _ => static_sqlite_error(MIGRATION_CONFIGURATION_MESSAGE),
    }
}

fn static_sqlite_error(message: &'static str) -> StateStoreError {
    StateStoreError::Sqlite {
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests;
