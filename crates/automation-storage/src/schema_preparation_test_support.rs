#![allow(clippy::unwrap_used)]

use sqlx::{
    Connection, SqliteConnection,
    migrate::Migrator,
    sqlite::{SqliteConnectOptions, SqliteJournalMode},
};
use std::{borrow::Cow, path::PathBuf};

pub(super) struct TemporaryAutomationDatabase {
    path: PathBuf,
}

impl TemporaryAutomationDatabase {
    pub(super) fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!(
                "automation-schema-preparation-{label}-{}.sqlite",
                uuid::Uuid::now_v7()
            )),
        }
    }

    pub(super) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TemporaryAutomationDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-journal", "-wal", "-shm"] {
            let sidecar = format!("{}{suffix}", self.path.display());
            let _ = std::fs::remove_file(sidecar);
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct DatabaseSnapshot {
    pub(super) main_database_bytes: Vec<u8>,
    pub(super) migration_history: Vec<(String, String, String, String, String, String)>,
    pub(super) schema_objects: Vec<(String, String, String, Option<String>)>,
    pub(super) user_version: i64,
    pub(super) event_count: i64,
    pub(super) preserved_event: Option<(i64, String, String, String, String, i64)>,
}

pub(super) async fn create_native_prefix_database(label: &str) -> TemporaryAutomationDatabase {
    let database = TemporaryAutomationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    let migrations = crate::schema_initialization::MIGRATOR
        .iter()
        .take(crate::schema_initialization::MIGRATOR.iter().count() - 1)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        migrations.last().map(|migration| migration.version),
        Some(20260928000000),
        "fixture must stop immediately before router-push creation"
    );
    let prefix_migrator = Migrator {
        migrations: Cow::Owned(migrations),
        ..Migrator::DEFAULT
    };
    prefix_migrator
        .run(&mut connection)
        .await
        .expect("actual automation migration prefix should apply");
    insert_preserved_event(&mut connection).await;
    connection
        .close()
        .await
        .expect("native-prefix fixture connection should close");
    database
}

pub(super) async fn create_full_native_database(label: &str) -> TemporaryAutomationDatabase {
    let database = TemporaryAutomationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    crate::schema_initialization::MIGRATOR
        .run(&mut connection)
        .await
        .expect("actual full automation migration set should apply");
    insert_preserved_event(&mut connection).await;
    connection
        .close()
        .await
        .expect("full-schema fixture connection should close");
    database
}

pub(super) async fn create_wal_current_native_database(label: &str) -> TemporaryAutomationDatabase {
    let database = create_native_prefix_database(label).await;
    let store = crate::AutomationStore::open(database.path())
        .await
        .expect("ordinary writer should complete the current automation schema");
    store
        .close()
        .await
        .expect("ordinary automation writer should close");
    enable_write_ahead_logging(database.path()).await;
    database
}

pub(super) async fn enable_write_ahead_logging(path: &std::path::Path) {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("fixture should enter write-ahead logging mode");
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await
        .expect("fixture journal mode should query");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    connection
        .close()
        .await
        .expect("WAL setup connection should close");
}

pub(super) async fn commit_schema_change_from_wal_writer(
    path: &std::path::Path,
    statement: &'static str,
) -> Result<(), String> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|error| error.to_string())?;
    let mut transaction = connection
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = sqlx::query(statement).execute(&mut *transaction).await {
        let _ = transaction.rollback().await;
        let _ = connection.close().await;
        return Err(error.to_string());
    }
    transaction
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    connection.close().await.map_err(|error| error.to_string())
}

async fn insert_preserved_event(connection: &mut SqliteConnection) {
    sqlx::query(
        "INSERT INTO automation_events(
            event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms
         ) VALUES(41,'prepare-event','run','prepare-run','accepted','{\"source\":\"fixture\"}',124)",
    )
    .execute(&mut *connection)
    .await
    .expect("domain event row should seed");
}

pub(super) async fn create_legacy_v1_database(label: &str) -> TemporaryAutomationDatabase {
    let database = TemporaryAutomationDatabase::new(label);
    let mut connection = open_writer(database.path(), true).await;
    sqlx::raw_sql(include_str!("../tests/fixtures/automation_v1.sql"))
        .execute(&mut connection)
        .await
        .expect("legacy v1 schema should seed");
    sqlx::query("PRAGMA user_version=1")
        .execute(&mut connection)
        .await
        .expect("legacy version should be recorded");
    connection
        .close()
        .await
        .expect("legacy fixture connection should close");
    database
}

pub(super) async fn replace_native_history_with_view(path: &std::path::Path) {
    let mut connection = open_writer(path, false).await;
    let history_rows: Vec<(i64, String, i64, Vec<u8>, i64)> = sqlx::query_as(
        "SELECT version,description,success,checksum,execution_time
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("authentic migration rows should query before replacing the table");
    let selects = history_rows
        .into_iter()
        .map(|(version, description, success, checksum, execution_time)| {
            let description = description.replace('\'', "''");
            let checksum = checksum
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>();
            format!(
                "SELECT {version} AS version,'{description}' AS description,{success} AS success,X'{checksum}' AS checksum,{execution_time} AS execution_time"
            )
        })
        .collect::<Vec<_>>();
    sqlx::query("DROP TABLE _sqlx_migrations")
        .execute(&mut connection)
        .await
        .expect("native history table should be replaced in the fixture");
    let view_sql = format!(
        "CREATE VIEW _sqlx_migrations AS {}",
        selects.join(" UNION ALL ")
    );
    sqlx::raw_sql(sqlx::AssertSqlSafe(view_sql))
        .execute(&mut connection)
        .await
        .expect("fixture view should project the authentic migration rows");
    connection
        .close()
        .await
        .expect("history view fixture should close");
}

pub(super) async fn open_writer(path: &std::path::Path, create: bool) -> SqliteConnection {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create)
        .foreign_keys(false)
        .journal_mode(SqliteJournalMode::Delete);
    SqliteConnection::connect_with(&options)
        .await
        .expect("test database should open")
}

pub(super) async fn capture_snapshot(path: &std::path::Path) -> DatabaseSnapshot {
    let main_database_bytes = std::fs::read(path).expect("main database bytes should read");
    let options = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false)
        .pragma("query_only", "ON");
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .expect("snapshot connection should open read-only");
    let migration_history = sqlx::query_as(
        "SELECT typeof(version),CAST(version AS TEXT),typeof(success),CAST(success AS TEXT),
                typeof(checksum),hex(checksum)
         FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("raw migration history should query");
    let schema_objects =
        sqlx::query_as("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")
            .fetch_all(&mut connection)
            .await
            .expect("schema objects should query");
    let user_version = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("user version should query");
    let event_count = sqlx::query_scalar("SELECT count(*) FROM automation_events")
        .fetch_one(&mut connection)
        .await
        .expect("domain event count should query");
    let preserved_event = sqlx::query_as(
        "SELECT event_sequence,event_id,subject_kind,subject_id,event_kind,recorded_at_ms
         FROM automation_events WHERE event_id='prepare-event'",
    )
    .fetch_optional(&mut connection)
    .await
    .expect("seeded event should query");
    connection
        .close()
        .await
        .expect("snapshot connection should close");
    DatabaseSnapshot {
        main_database_bytes,
        migration_history,
        schema_objects,
        user_version,
        event_count,
        preserved_event,
    }
}

pub(super) async fn apply_history_defect(path: &std::path::Path, defect: super::HistoryDefect) {
    let mut connection = open_writer(path, false).await;
    let first_version = crate::schema_initialization::MIGRATOR
        .iter()
        .next()
        .expect("image has an initial migration")
        .version;
    let newest_version = crate::schema_initialization::MIGRATOR
        .iter()
        .last()
        .expect("image has a newest migration")
        .version;
    if matches!(
        defect,
        super::HistoryDefect::InvalidVersionStorageClass
            | super::HistoryDefect::NullVersion
            | super::HistoryDefect::InvalidSuccessStorageClass
    ) {
        rebuild_migration_history_table(&mut connection).await;
    }
    match defect {
        super::HistoryDefect::Dirty => {
            sqlx::query("UPDATE _sqlx_migrations SET success=0 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration should become dirty");
        }
        super::HistoryDefect::ChecksumMismatch => {
            sqlx::query("UPDATE _sqlx_migrations SET checksum=X'00' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration checksum should change");
        }
        super::HistoryDefect::ChecksumStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET checksum='not-a-blob' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration checksum storage class should change");
        }
        super::HistoryDefect::NewerThanImage => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
                 VALUES(?,'future migration',1,X'00',1)",
            )
            .bind(newest_version + 1)
            .execute(&mut connection)
            .await
            .expect("newer migration should insert");
        }
        super::HistoryDefect::UnknownMigration => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
                 VALUES(20260925000000,'unknown migration',1,X'00',1)",
            )
            .execute(&mut connection)
            .await
            .expect("unknown migration should insert");
        }
        super::HistoryDefect::InvalidOrder => {
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("native history should stop being a prefix");
        }
        super::HistoryDefect::InvalidVersion => {
            sqlx::query("UPDATE _sqlx_migrations SET version=0 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version should become invalid");
        }
        super::HistoryDefect::InvalidVersionStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET version='not-an-integer' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version storage class should change");
        }
        super::HistoryDefect::NullVersion => {
            sqlx::query("UPDATE _sqlx_migrations SET version=NULL WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration version should become NULL");
        }
        super::HistoryDefect::InvalidSuccess => {
            sqlx::query("UPDATE _sqlx_migrations SET success=2 WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration success should become nonboolean");
        }
        super::HistoryDefect::InvalidSuccessStorageClass => {
            sqlx::query("UPDATE _sqlx_migrations SET success='not-a-boolean' WHERE version=?")
                .bind(first_version)
                .execute(&mut connection)
                .await
                .expect("migration success storage class should change");
        }
    }
    connection
        .close()
        .await
        .expect("corrupted history connection should close");
}

async fn rebuild_migration_history_table(connection: &mut SqliteConnection) {
    sqlx::query("DROP TABLE _sqlx_migrations")
        .execute(&mut *connection)
        .await
        .expect("native migration history table should drop in fixture");
    sqlx::query(
        "CREATE TABLE _sqlx_migrations(
            version INTEGER, description TEXT, success BOOLEAN, checksum BLOB, execution_time BIGINT
         )",
    )
    .execute(&mut *connection)
    .await
    .expect("fixture should recreate history with permissive test columns");
    for migration in crate::schema_initialization::MIGRATOR.iter() {
        sqlx::query(
            "INSERT INTO _sqlx_migrations(version,description,success,checksum,execution_time)
             VALUES(?,?,1,?,0)",
        )
        .bind(migration.version)
        .bind(migration.description.as_ref())
        .bind(migration.checksum.as_ref())
        .execute(&mut *connection)
        .await
        .expect("real migration metadata should reseed history");
    }
}
