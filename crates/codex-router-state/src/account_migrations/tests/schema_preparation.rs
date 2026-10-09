use super::*;
use crate::schema_preparation::AccountMigrationVersion;
use crate::schema_preparation::AccountSchemaPreparation;
use crate::schema_preparation::StateSchemaPreparationError;
use sqlx::migrate::Migrator;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions, sqlite::SqliteJournalMode};
use std::{borrow::Cow, path::Path};

async fn create_native_prefix_database(label: &str) -> TemporaryDatabase {
    let database = TemporaryDatabase::new(label);
    let migrations = MIGRATOR.iter().cloned().collect::<Vec<_>>();
    assert!(migrations.len() > 1, "fixture needs a pending migration");
    let prefix = Migrator {
        migrations: Cow::Owned(migrations[..migrations.len() - 1].to_vec()),
        ..Migrator::DEFAULT
    };
    let mut connection = open_test_connection(database.path(), true).await;
    prefix
        .run(&mut connection)
        .await
        .expect("real migration prefix should initialize");
    sqlx::query(
        "INSERT INTO accounts (
            account_id, label, status, active_credential_generation, provider
         ) VALUES ('preserved-account', 'before-prepare', 'disabled', 41, 'openai')",
    )
    .execute(&mut connection)
    .await
    .expect("existing account row should seed");
    connection
        .close()
        .await
        .expect("prefix fixture connection should close");
    database
}

async fn native_prefix_snapshot(database_path: &Path) -> NativePrefixSnapshot {
    let main_database_bytes =
        std::fs::read(database_path).expect("main database bytes should read");
    let mut connection = open_test_connection(database_path, false).await;
    let migration_history: Vec<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT typeof(version), CAST(version AS TEXT),
                typeof(success), CAST(success AS TEXT),
                typeof(checksum), hex(checksum)
           FROM _sqlx_migrations ORDER BY version",
    )
    .fetch_all(&mut connection)
    .await
    .expect("raw migration history should query");
    let schema_objects: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name")
            .fetch_all(&mut connection)
            .await
            .expect("schema objects should query");
    let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("user version should query");
    let preserved_account: (String, String, String, Option<i64>, String) = sqlx::query_as(
        "SELECT account_id, label, status, active_credential_generation, provider
           FROM accounts WHERE account_id = 'preserved-account'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("preserved account should query");
    connection
        .close()
        .await
        .expect("snapshot connection should close");
    NativePrefixSnapshot {
        main_database_bytes,
        migration_history,
        schema_objects,
        user_version,
        preserved_account,
    }
}

async fn commit_account_schema_change_from_wal_writer(database_path: &Path) -> Result<(), String> {
    let options = SqliteConnectOptions::new()
        .filename(database_path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Wal);
    let mut connection = SqliteConnection::connect_with(&options)
        .await
        .map_err(|error| error.to_string())?;
    let mut transaction = connection
        .begin()
        .await
        .map_err(|error| error.to_string())?;
    if let Err(error) = sqlx::query("ALTER TABLE accounts ADD COLUMN snapshot_probe TEXT")
        .execute(&mut *transaction)
        .await
    {
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

#[derive(Debug, Eq, PartialEq)]
struct NativePrefixSnapshot {
    main_database_bytes: Vec<u8>,
    migration_history: Vec<(String, String, String, String, String, String)>,
    schema_objects: Vec<(String, String, String, Option<String>)>,
    user_version: i64,
    preserved_account: (String, String, String, Option<i64>, String),
}

#[tokio::test]
async fn read_only_reporting_stays_strict_for_a_native_prefix() {
    let database = create_native_prefix_database("reporting_native_prefix").await;
    let before = native_prefix_snapshot(database.path()).await;

    let error = AsyncSqliteStateStore::open_read_only(database.path())
        .await
        .expect_err("status opener must still reject pending native migrations");
    assert!(format!("{error}").contains("writable migration"));

    assert_eq!(native_prefix_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_returns_the_ordered_pending_suffix_without_mutation() {
    let database = create_native_prefix_database("prepare_pending_prefix").await;
    let before = native_prefix_snapshot(database.path()).await;
    let pending_versions = MIGRATOR
        .iter()
        .skip(MIGRATOR.iter().count() - 1)
        .map(|migration| {
            AccountMigrationVersion::new(migration.version)
                .expect("embedded migration versions should be positive")
        })
        .collect::<Vec<_>>();

    let preparation = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect("real native prefix should produce a pending result");
    match preparation {
        AccountSchemaPreparation::Pending { migrations } => {
            assert_eq!(migrations.as_slice(), pending_versions);
        }
        AccountSchemaPreparation::Current => {
            panic!("native prefix must report the exact pending suffix");
        }
    }

    assert_eq!(native_prefix_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_reports_current_without_mutating_the_database() {
    let database = create_native_prefix_database("prepare_current").await;
    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("ordinary writer should apply the remaining migration");
    writer.close().await.expect("ordinary writer should close");
    let before = native_prefix_snapshot(database.path()).await;

    assert!(matches!(
        AsyncSqliteStateStore::prepare_schema(database.path()).await,
        Ok(AccountSchemaPreparation::Current)
    ));

    assert_eq!(native_prefix_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn prepare_schema_keeps_history_and_current_validation_in_one_wal_snapshot() {
    let database = create_native_prefix_database("prepare_wal_snapshot").await;
    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("ordinary writer should finish the current schema");
    writer.close().await.expect("ordinary writer should close");
    let mut wal_check = open_test_connection(database.path(), false).await;
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut wal_check)
        .await
        .expect("state database journal mode should query");
    assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    wal_check.close().await.expect("WAL check should close");
    let before = native_prefix_snapshot(database.path()).await;
    assert!(
        !before
            .schema_objects
            .iter()
            .any(|(_, name, _, definition)| name == "accounts"
                && definition
                    .as_deref()
                    .is_some_and(|sql| sql.contains("snapshot_probe")))
    );

    let database_path = database.path().to_path_buf();
    let (history_read_sender, history_read_receiver) = tokio::sync::oneshot::channel();
    let (writer_result_sender, writer_result_receiver) = tokio::sync::oneshot::channel();
    let writer_path = database_path.clone();
    let writer_task = tokio::spawn(async move {
        history_read_receiver
            .await
            .expect("inspector should reach the post-history checkpoint");
        let result = commit_account_schema_change_from_wal_writer(&writer_path).await;
        writer_result_sender
            .send(result.clone())
            .expect("inspector should receive writer completion");
        result
    });

    let preparation = crate::schema_preparation::prepare_schema_with_checkpoint(
        &database_path,
        move || async move {
            history_read_sender
                .send(())
                .expect("WAL writer should await native history read");
            writer_result_receiver
                .await
                .expect("WAL writer should report its commit")
                .expect("WAL writer should add its fixture-only column");
        },
    )
    .await;
    writer_task
        .await
        .expect("external schema writer should finish")
        .expect("external schema writer should commit");
    assert!(
        matches!(&preparation, Ok(AccountSchemaPreparation::Current)),
        "first inspection should use its original SQLite snapshot: {preparation:?}"
    );

    let after_writer_commit = native_prefix_snapshot(&database_path).await;
    assert_eq!(
        after_writer_commit.migration_history,
        before.migration_history
    );
    assert_eq!(after_writer_commit.user_version, before.user_version);
    assert_eq!(
        after_writer_commit.preserved_account,
        before.preserved_account
    );
    assert!(
        after_writer_commit
            .schema_objects
            .iter()
            .any(|(_, name, _, definition)| name == "accounts"
                && definition
                    .as_deref()
                    .is_some_and(|sql| sql.contains("snapshot_probe")))
    );

    let error = AsyncSqliteStateStore::prepare_schema(&database_path)
        .await
        .expect_err("a later inspection should observe the committed schema change");
    assert!(matches!(
        error,
        StateSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    let after_later_inspection = native_prefix_snapshot(&database_path).await;
    // A WAL checkpoint may move the committed fixture DDL into the main file.
    assert_eq!(
        after_later_inspection.migration_history,
        after_writer_commit.migration_history
    );
    assert_eq!(
        after_later_inspection.schema_objects,
        after_writer_commit.schema_objects
    );
    assert_eq!(
        after_later_inspection.user_version,
        after_writer_commit.user_version
    );
    assert_eq!(
        after_later_inspection.preserved_account,
        after_writer_commit.preserved_account
    );
}

#[tokio::test]
async fn prepare_schema_migration_result_connects_to_the_ordinary_writer() {
    let database = create_native_prefix_database("prepare_then_activate").await;
    let pending = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect("native prefix should prepare");
    assert!(matches!(pending, AccountSchemaPreparation::Pending { .. }));

    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("ordinary writer should apply the pending migration");
    let account = writer
        .load_account(
            &codex_router_core::ids::AccountId::new("preserved-account")
                .expect("fixture account id"),
        )
        .await
        .expect("preserved account should load")
        .expect("preserved account should remain");
    assert_eq!(account.status(), crate::account::AccountStatus::Disabled);
    assert_eq!(account.active_credential_generation(), Some(41));
    writer.close().await.expect("ordinary writer should close");

    assert!(matches!(
        AsyncSqliteStateStore::prepare_schema(database.path()).await,
        Ok(AccountSchemaPreparation::Current)
    ));
}

#[tokio::test]
async fn prepare_schema_does_not_create_a_missing_database() {
    let database = TemporaryDatabase::new("prepare_missing_database");
    assert!(!database.path().exists());

    let error = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect_err("missing database must be rejected");

    assert!(matches!(
        error,
        StateSchemaPreparationError::UnreadableStore { .. }
    ));
    assert!(!database.path().exists());
}

#[tokio::test]
async fn prepare_schema_rejects_legacy_history_without_mutating_it() {
    let database = TemporaryDatabase::new("prepare_legacy_history");
    create_legacy_v13_database(database.path(), "legacy-preparation").await;
    let before = std::fs::read(database.path()).expect("legacy database bytes should read");

    let error = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect_err("legacy history must not be adopted during preparation");

    assert!(matches!(
        error,
        StateSchemaPreparationError::UnrecognizedNativeHistory
    ));
    assert_eq!(
        std::fs::read(database.path()).expect("legacy database bytes should read"),
        before
    );
    assert_legacy_database_unchanged(database.path(), "legacy-preparation").await;
}

#[derive(Clone, Copy)]
enum HistoryDefect {
    Dirty,
    ChecksumMismatch,
    NewerThanImage,
    UnknownMigration,
    InvalidOrder,
    InvalidVersion,
    InvalidSuccess,
}

fn matches_history_defect(error: StateSchemaPreparationError, defect: HistoryDefect) -> bool {
    matches!(
        (error, defect),
        (
            StateSchemaPreparationError::DirtyMigration,
            HistoryDefect::Dirty
        ) | (
            StateSchemaPreparationError::ChecksumMismatch,
            HistoryDefect::ChecksumMismatch
        ) | (
            StateSchemaPreparationError::SchemaNewerThanImage,
            HistoryDefect::NewerThanImage
        ) | (
            StateSchemaPreparationError::UnknownAppliedMigration,
            HistoryDefect::UnknownMigration
        ) | (
            StateSchemaPreparationError::InvalidAppliedOrder,
            HistoryDefect::InvalidOrder
        ) | (
            StateSchemaPreparationError::InvalidMigrationVersion,
            HistoryDefect::InvalidVersion
        ) | (
            StateSchemaPreparationError::InvalidMigrationHistory,
            HistoryDefect::InvalidSuccess
        )
    )
}

async fn apply_history_defect(database_path: &Path, defect: HistoryDefect) {
    let mut connection = open_test_connection(database_path, false).await;
    let migration_version = MIGRATOR
        .iter()
        .next()
        .expect("image should contain migrations")
        .version;
    let newest_version = MIGRATOR
        .iter()
        .last()
        .expect("image should contain migrations")
        .version;
    match defect {
        HistoryDefect::Dirty => {
            sqlx::query("UPDATE _sqlx_migrations SET success = 0 WHERE version = ?1")
                .bind(migration_version)
                .execute(&mut connection)
                .await
                .expect("history should become dirty");
        }
        HistoryDefect::ChecksumMismatch => {
            sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00' WHERE version = ?1")
                .bind(migration_version)
                .execute(&mut connection)
                .await
                .expect("history checksum should change");
        }
        HistoryDefect::NewerThanImage => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations (
                    version, description, success, checksum, execution_time
                 ) VALUES (?1, 'future migration', 1, X'00', 1)",
            )
            .bind(newest_version + 1)
            .execute(&mut connection)
            .await
            .expect("newer migration row should insert");
        }
        HistoryDefect::UnknownMigration => {
            sqlx::query(
                "INSERT INTO _sqlx_migrations (
                    version, description, success, checksum, execution_time
                 ) VALUES (202609260001, 'unknown migration', 1, X'00', 1)",
            )
            .execute(&mut connection)
            .await
            .expect("unknown migration row should insert");
        }
        HistoryDefect::InvalidOrder => {
            sqlx::query("DELETE FROM _sqlx_migrations WHERE version = ?1")
                .bind(migration_version)
                .execute(&mut connection)
                .await
                .expect("history prefix should become invalid");
        }
        HistoryDefect::InvalidVersion => {
            sqlx::query("UPDATE _sqlx_migrations SET version = 0 WHERE version = ?1")
                .bind(migration_version)
                .execute(&mut connection)
                .await
                .expect("history version should become invalid");
        }
        HistoryDefect::InvalidSuccess => {
            sqlx::query("UPDATE _sqlx_migrations SET success = 2 WHERE version = ?1")
                .bind(migration_version)
                .execute(&mut connection)
                .await
                .expect("history success value should become nonboolean");
        }
    }
    connection
        .close()
        .await
        .expect("corrupt history connection should close");
}

#[tokio::test]
async fn prepare_schema_rejects_invalid_native_history_without_repair() {
    for (sequence, defect) in [
        HistoryDefect::Dirty,
        HistoryDefect::ChecksumMismatch,
        HistoryDefect::NewerThanImage,
        HistoryDefect::UnknownMigration,
        HistoryDefect::InvalidOrder,
        HistoryDefect::InvalidVersion,
        HistoryDefect::InvalidSuccess,
    ]
    .into_iter()
    .enumerate()
    {
        let database = create_native_prefix_database(&format!("prepare_invalid_{sequence}")).await;
        if matches!(
            defect,
            HistoryDefect::NewerThanImage
                | HistoryDefect::UnknownMigration
                | HistoryDefect::InvalidOrder
                | HistoryDefect::InvalidVersion
                | HistoryDefect::InvalidSuccess
        ) {
            let writer = AsyncSqliteStateStore::open(database.path())
                .await
                .expect("ordinary writer should complete the prefix before corruption");
            writer.close().await.expect("ordinary writer should close");
        }
        apply_history_defect(database.path(), defect).await;
        let before = native_prefix_snapshot(database.path()).await;

        let error = AsyncSqliteStateStore::prepare_schema(database.path())
            .await
            .expect_err("invalid migration history must be rejected");
        assert!(matches_history_defect(error, defect));
        assert_eq!(native_prefix_snapshot(database.path()).await, before);
    }
}

#[tokio::test]
async fn prepare_schema_rejects_a_damaged_current_schema_without_repair() {
    let database = create_native_prefix_database("prepare_damaged_schema").await;
    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("ordinary writer should create the current schema");
    writer.close().await.expect("ordinary writer should close");
    let mut connection = open_test_connection(database.path(), false).await;
    sqlx::query("DROP TABLE account_credit_observations")
        .execute(&mut connection)
        .await
        .expect("current schema table should be damaged");
    connection
        .close()
        .await
        .expect("damaged schema connection should close");
    let before = native_prefix_snapshot(database.path()).await;

    let error = AsyncSqliteStateStore::prepare_schema(database.path())
        .await
        .expect_err("current history with damaged schema must be rejected");

    assert!(matches!(
        error,
        StateSchemaPreparationError::InvalidCurrentSchema { .. }
    ));
    assert_eq!(native_prefix_snapshot(database.path()).await, before);
}

#[tokio::test]
async fn reporting_retains_nonzero_success_interpretation() {
    let database = create_native_prefix_database("reporting_nonzero_success").await;
    let writer = AsyncSqliteStateStore::open(database.path())
        .await
        .expect("ordinary writer should create the current schema");
    writer.close().await.expect("ordinary writer should close");
    let mut connection = open_test_connection(database.path(), false).await;
    sqlx::query("UPDATE _sqlx_migrations SET success = 2 WHERE version = ?1")
        .bind(MIGRATOR.iter().next().expect("image migration").version)
        .execute(&mut connection)
        .await
        .expect("nonzero success value should store");
    connection
        .close()
        .await
        .expect("nonzero history connection should close");

    let reporting = AsyncSqliteStateStore::open_read_only(database.path())
        .await
        .expect("existing reader keeps SQLx nonzero-bool behavior");
    reporting
        .close()
        .await
        .expect("reporting reader should close");
    assert!(matches!(
        AsyncSqliteStateStore::prepare_schema(database.path()).await,
        Err(StateSchemaPreparationError::InvalidMigrationHistory)
    ));
}

#[test]
fn account_migration_version_accepts_only_positive_values() {
    assert_eq!(
        AccountMigrationVersion::new(1).map(AccountMigrationVersion::get),
        Some(1)
    );
    assert!(AccountMigrationVersion::new(0).is_none());
    assert!(AccountMigrationVersion::new(-1).is_none());
}
