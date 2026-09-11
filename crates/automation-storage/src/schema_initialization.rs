//! Native SQLx schema migration and bounded adoption of the exact legacy v1 schema.
use crate::{StorageError, schema_validation};
use sqlx::{Connection, SqliteConnection, migrate::MigrateError};

const BASELINE_VERSION: i64 = 20_260_910_000_000;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StorageError> {
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    let has_native_history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(&mut *transaction)
    .await?;

    if has_native_history {
        MIGRATOR
            .run_direct(None, &mut *transaction, false)
            .await
            .map_err(map_migrate_error)?;
    } else {
        let legacy_version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut *transaction)
            .await?;
        match legacy_version {
            0 => {
                if schema_validation::has_domain_objects(&mut transaction).await? {
                    return Err(StorageError::InvalidSchema);
                }
                MIGRATOR
                    .run_direct(None, &mut *transaction, false)
                    .await
                    .map_err(map_migrate_error)?;
                migration_test_checkpoint("baseline-registered");
            }
            1 => {
                schema_validation::validate_target_schema(&mut transaction).await?;
                MIGRATOR
                    .run_direct(Some(BASELINE_VERSION), &mut *transaction, true)
                    .await
                    .map_err(map_migrate_error)?;
                migration_test_checkpoint("baseline-registered");
                MIGRATOR
                    .run_direct(None, &mut *transaction, false)
                    .await
                    .map_err(map_migrate_error)?;
            }
            other => return Err(StorageError::UnsupportedVersion(other)),
        }
    }

    schema_validation::validate_target_schema(&mut transaction).await?;
    migration_test_checkpoint("before-outer-commit");
    transaction.commit().await?;
    migration_test_checkpoint("outer-committed");
    Ok(())
}

#[cfg(not(test))]
fn migration_test_checkpoint(_stage: &str) {}

#[cfg(test)]
fn migration_test_checkpoint(stage: &str) {
    if std::env::var(tests::CHECKPOINT_STAGE_ENV).as_deref() == Ok(stage)
        && tests::child_database_path().is_ok_and(|path| path.is_some())
    {
        std::process::exit(tests::CHECKPOINT_EXIT_CODE);
    }
}

#[allow(deprecated)]
fn map_migrate_error(error: MigrateError) -> StorageError {
    match error {
        MigrateError::Execute(error) | MigrateError::ExecuteMigration(error, _) => {
            StorageError::Database(error)
        }
        MigrateError::Source(_)
        | MigrateError::VersionMissing(_)
        | MigrateError::VersionMismatch(_)
        | MigrateError::VersionNotPresent(_)
        | MigrateError::VersionTooOld(_, _)
        | MigrateError::VersionTooNew(_, _)
        | MigrateError::ForceNotSupported
        | MigrateError::InvalidMixReversibleAndSimple
        | MigrateError::Dirty(_)
        | MigrateError::CreateSchemasNotSupported(_)
        | MigrateError::SkipNotSupported() => StorageError::InvalidSchema,
        _ => StorageError::InvalidSchema,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    pub(super) const CHECKPOINT_STAGE_ENV: &str = "AUTOMATION_MIGRATION_TEST_CHECKPOINT";
    const DATABASE_PATH_ENV: &str = "AUTOMATION_MIGRATION_TEST_DATABASE";
    pub(super) const CHECKPOINT_EXIT_CODE: i32 = 93;
    const LEGACY_V1_SCHEMA: &str = include_str!("../tests/fixtures/automation_v1.sql");
    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

    struct TestDatabase {
        path: PathBuf,
    }

    impl TestDatabase {
        fn new(label: &str) -> Self {
            Self {
                path: std::env::temp_dir().join(format!(
                    "automation-migration-crash-{label}-{}.sqlite",
                    uuid::Uuid::now_v7()
                )),
            }
        }
    }

    impl Drop for TestDatabase {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    pub(super) fn child_database_path() -> TestResult<Option<PathBuf>> {
        let Some(path) = std::env::var_os(DATABASE_PATH_ENV).map(PathBuf::from) else {
            return Ok(None);
        };
        if path.parent() != Some(std::env::temp_dir().as_path())
            || !path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("automation-migration-crash-")
            })
        {
            return Err("refusing non-fixture automation migration path".into());
        }
        Ok(Some(path))
    }

    async fn connect(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
        SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(path)
                .create_if_missing(true)
                .foreign_keys(true),
        )
        .await
    }

    async fn create_seeded_legacy_v1(path: &Path) -> TestResult {
        let mut connection = connect(path).await?;
        sqlx::raw_sql(sqlx::AssertSqlSafe(LEGACY_V1_SCHEMA))
            .execute(&mut connection)
            .await?;
        sqlx::query("PRAGMA user_version=1")
            .execute(&mut connection)
            .await?;
        sqlx::query("INSERT INTO automation_events(event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (41,'crash-sentinel','run','run-1','uncertain','{\"nativeInputAccepted\":true}',124)").execute(&mut connection).await?;
        sqlx::query("INSERT INTO automation_events(event_sequence,event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (79,'temporary-high-water','run','run-1','temporary','{}',125)").execute(&mut connection).await?;
        sqlx::query("DELETE FROM automation_events WHERE event_id='temporary-high-water'")
            .execute(&mut connection)
            .await?;
        connection.close().await?;
        Ok(())
    }

    async fn run_checkpoint_child(path: &Path, stage: &str) -> TestResult {
        let output = tokio::time::timeout(
            Duration::from_secs(15),
            tokio::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "schema_initialization::tests::migration_checkpoint_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env(DATABASE_PATH_ENV, path)
                .env(CHECKPOINT_STAGE_ENV, stage)
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        if output.status.code() != Some(CHECKPOINT_EXIT_CODE) {
            return Err(format!(
                "migration child missed {stage}: {:?}; {}; {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(())
    }

    async fn inspect_seed_and_history(path: &Path) -> TestResult<(i64, i64, i64, Vec<Vec<u8>>)> {
        let mut connection = connect(path).await?;
        let event_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM automation_events WHERE event_id='crash-sentinel' AND event_body_json='{\"nativeInputAccepted\":true}'",
        )
        .fetch_one(&mut connection)
        .await?;
        let high_water: i64 =
            sqlx::query_scalar("SELECT seq FROM sqlite_sequence WHERE name='automation_events'")
                .fetch_one(&mut connection)
                .await?;
        let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&mut connection)
            .await?;
        let history_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
        )
        .fetch_one(&mut connection)
        .await?;
        let history = if history_exists {
            sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&mut connection)
                .await?
        } else {
            Vec::new()
        };
        connection.close().await?;
        Ok((event_count, high_water, user_version, history))
    }

    #[test]
    fn migration_execution_errors_preserve_database_category() {
        let execute = map_migrate_error(MigrateError::Execute(sqlx::Error::Protocol(
            "injected execution failure".into(),
        )));
        assert!(matches!(execute, StorageError::Database(_)));
        let migration = map_migrate_error(MigrateError::ExecuteMigration(
            sqlx::Error::Protocol("injected migration failure".into()),
            BASELINE_VERSION,
        ));
        assert!(matches!(migration, StorageError::Database(_)));
    }

    #[test]
    fn incompatible_migration_history_preserves_schema_category() {
        let errors = [
            MigrateError::VersionMissing(BASELINE_VERSION),
            MigrateError::VersionMismatch(BASELINE_VERSION),
            MigrateError::VersionNotPresent(BASELINE_VERSION),
            MigrateError::VersionTooOld(BASELINE_VERSION, BASELINE_VERSION + 1),
            MigrateError::VersionTooNew(BASELINE_VERSION, BASELINE_VERSION - 1),
            MigrateError::Dirty(BASELINE_VERSION),
            MigrateError::SkipNotSupported(),
        ];
        for error in errors {
            assert!(matches!(
                map_migrate_error(error),
                StorageError::InvalidSchema
            ));
        }
    }

    #[tokio::test]
    async fn outer_managed_transaction_rolls_back_nested_native_migration() {
        let mut connection =
            SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(":memory:"))
                .await
                .expect("in-memory SQLite should connect");
        let mut transaction = connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .expect("managed writer transaction should begin");
        MIGRATOR
            .run_direct(None, &mut *transaction, false)
            .await
            .expect("nested native migration should apply");
        drop(transaction);

        let domain_tables: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_one(&mut connection)
        .await
        .expect("connection should observe queued rollback");
        assert_eq!(domain_tables, 0);
    }

    #[tokio::test]
    async fn process_exit_before_outer_commit_rolls_back_adoption() -> TestResult {
        for stage in ["baseline-registered", "before-outer-commit"] {
            let database = TestDatabase::new(stage);
            create_seeded_legacy_v1(&database.path).await?;
            run_checkpoint_child(&database.path, stage).await?;

            let original = inspect_seed_and_history(&database.path).await?;
            if original != (1, 79, 1, Vec::new()) {
                return Err(
                    format!("{stage}: precommit exit changed legacy state: {original:?}").into(),
                );
            }
            crate::AutomationStore::open(&database.path)
                .await?
                .close()
                .await?;
            let adopted = inspect_seed_and_history(&database.path).await?;
            if adopted.0 != 1 || adopted.1 != 79 || adopted.2 != 1 || adopted.3.len() != 1 {
                return Err(
                    format!("{stage}: reopen did not adopt intact state: {adopted:?}").into(),
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn process_exit_after_commit_reopens_without_replaying_adoption() -> TestResult {
        let database = TestDatabase::new("postcommit");
        create_seeded_legacy_v1(&database.path).await?;
        run_checkpoint_child(&database.path, "outer-committed").await?;
        let committed = inspect_seed_and_history(&database.path).await?;
        if committed.0 != 1 || committed.1 != 79 || committed.2 != 1 || committed.3.len() != 1 {
            return Err(format!("postcommit exit lost native state: {committed:?}").into());
        }
        crate::AutomationStore::open(&database.path)
            .await?
            .close()
            .await?;
        let reopened = inspect_seed_and_history(&database.path).await?;
        if reopened != committed {
            return Err("postcommit reopen replayed adoption or changed domain state".into());
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "owned subprocess exits at the selected migration checkpoint"]
    async fn migration_checkpoint_child() -> TestResult {
        let path = child_database_path()?.ok_or("missing migration child database")?;
        crate::AutomationStore::open(&path).await?.close().await?;
        Err("selected migration checkpoint was not reached".into())
    }
}
