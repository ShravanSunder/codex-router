use super::*;
use std::path::Path;

fn identity() -> Result<UuidIdentity, Box<dyn std::error::Error>> {
    Ok("00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()?)
}

async fn connect(path: &Path) -> Result<SqliteConnection, sqlx::Error> {
    SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
}

#[tokio::test]
async fn interrupted_adoption_rolls_back_native_history_and_reopens() {
    for legacy in [false, true] {
        let path = std::env::temp_dir().join(format!(
            "journal-crash-{}-{legacy}.sqlite",
            std::process::id()
        ));
        let mut connection = connect(&path).await.unwrap();
        if legacy {
            sqlx::raw_sql(include_str!("../../tests/fixtures/journal_v1.sql"))
                .execute(&mut connection)
                .await
                .unwrap();
            sqlx::raw_sql("INSERT INTO journal_metadata VALUES(1,1,'00000000-0000-4000-8000-000000000001',12,90,55); INSERT INTO journal_checkpoint VALUES(1,8);")
                .execute(&mut connection).await.unwrap();
        }
        connection.close().await.unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "journal_migrations::tests::crash_before_commit_child",
                "--nocapture",
            ])
            .env("JOURNAL_MIGRATION_CRASH_DATABASE", &path)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(73));
        let mut connection = connect(&path).await.unwrap();
        let registered: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_sqlx_migrations')",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        assert!(
            !registered,
            "process exit must roll back baseline registration"
        );
        if legacy {
            let counters: (i64, i64, i64) = sqlx::query_as(
                "SELECT last_sequence,retention_clock,payload_bytes FROM journal_metadata",
            )
            .fetch_one(&mut connection)
            .await
            .unwrap();
            assert_eq!(counters, (12, 90, 55));
        }
        initialize(&mut connection, identity().unwrap())
            .await
            .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success=TRUE")
                .fetch_one(&mut connection)
                .await
                .unwrap();
        assert_eq!(count, 1);
        connection.close().await.unwrap();
        std::fs::remove_file(path).unwrap();
    }
}

#[tokio::test]
async fn crash_before_commit_child() {
    let Some(path) = std::env::var_os("JOURNAL_MIGRATION_CRASH_DATABASE") else {
        return;
    };
    let mut connection = connect(Path::new(&path)).await.unwrap();
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
    initialize_transaction(
        &mut transaction,
        identity().unwrap(),
        &MIGRATOR,
        CURRENT_SCHEMA_SQL,
    )
    .await
    .unwrap();
    // Abrupt exit deliberately bypasses Rust destructors after SQLx has written
    // its history, but before the owning journal transaction commits.
    std::process::exit(73);
}

#[tokio::test]
async fn subsequent_migration_uses_current_target_without_rewriting_baseline() {
    use sqlx::migrate::{Migration, MigrationType};
    use std::borrow::Cow;
    const UPGRADE_SQL: &str = "CREATE TABLE future_journal_state (value TEXT NOT NULL);";
    let mut connection = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    initialize(&mut connection, identity().unwrap())
        .await
        .unwrap();
    let baseline_checksum: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let mut migrations = MIGRATOR.iter().cloned().collect::<Vec<_>>();
    migrations.push(Migration::new(
        BASELINE_VERSION + 1,
        Cow::Borrowed("future journal state"),
        MigrationType::Simple,
        sqlx::SqlStr::from_static(UPGRADE_SQL),
        false,
    ));
    let upgraded = sqlx::migrate::Migrator::with_migrations(migrations);
    let target = format!("{CURRENT_SCHEMA_SQL}\n{UPGRADE_SQL}");
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
    initialize_transaction(&mut transaction, identity().unwrap(), &upgraded, &target)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let history: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version,checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0], (BASELINE_VERSION, baseline_checksum));
    sqlx::query("INSERT INTO future_journal_state VALUES ('available')")
        .execute(&mut connection)
        .await
        .unwrap();
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await.unwrap();
    initialize_transaction(&mut transaction, identity().unwrap(), &upgraded, &target)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let value: String = sqlx::query_scalar("SELECT value FROM future_journal_state")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(value, "available");
}
