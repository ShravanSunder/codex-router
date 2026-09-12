//! SQLx owns migration history; foreign keys are checked before commit and enabled before use.
use crate::BoardStorageError;
use sqlx::{Connection, Row, SqliteConnection};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const BASELINE: &str = include_str!("../migrations/202609120001_project_board.sql");

pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), BoardStorageError> {
    initialize_with(connection, &MIGRATOR, BASELINE).await
}

async fn initialize_with(
    connection: &mut SqliteConnection,
    migrator: &sqlx::migrate::Migrator,
    expected_schema: &str,
) -> Result<(), BoardStorageError> {
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    let has_history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='_sqlx_migrations')",
    )
    .fetch_one(&mut *transaction)
    .await?;
    let has_objects: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name!='_sqlx_migrations')")
        .fetch_one(&mut *transaction).await?;
    if has_objects && !has_history {
        return Err(BoardStorageError::InvalidSchema);
    }
    migrator
        .run_direct(None, &mut *transaction, false)
        .await
        .map_err(|_| BoardStorageError::InvalidSchema)?;
    if !sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&mut *transaction)
        .await?
        .is_empty()
    {
        return Err(BoardStorageError::InvalidSchema);
    }
    validate_schema(&mut transaction, expected_schema).await?;
    let checkpoint: i64 = sqlx::query_scalar!(
        "SELECT count(*) FROM activity_checkpoint WHERE singleton=1 AND last_sequence>=0 AND length(cursor_key)=32 AND (SELECT count(*) FROM activity_checkpoint)=1"
    )
    .fetch_one(&mut *transaction)
    .await?;
    if checkpoint != 1 {
        return Err(BoardStorageError::InvalidSchema);
    }
    transaction.commit().await?;
    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&mut *connection)
        .await?;
    let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(connection)
        .await?;
    if enabled != 1 {
        return Err(BoardStorageError::InvalidSchema);
    }
    Ok(())
}

async fn definitions(
    connection: &mut SqliteConnection,
) -> Result<Vec<(String, String)>, BoardStorageError> {
    sqlx::query("SELECT name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name!='_sqlx_migrations' ORDER BY name")
        .fetch_all(connection).await?.into_iter().map(|row| {
            let name: String = row.try_get("name")?;
            let sql: String = row.try_get("sql")?;
            Ok((name,sql.split_whitespace().collect::<Vec<_>>().join(" ")))
        }).collect()
}
async fn validate_schema(
    connection: &mut SqliteConnection,
    expected_schema: &str,
) -> Result<(), BoardStorageError> {
    let mut expected = SqliteConnection::connect("sqlite::memory:").await?;
    sqlx::raw_sql(sqlx::AssertSqlSafe(expected_schema))
        .execute(&mut expected)
        .await?;
    let expected_objects = definitions(&mut expected).await?;
    let actual_objects = definitions(connection).await?;
    expected.close().await?;
    if actual_objects != expected_objects {
        return Err(BoardStorageError::InvalidSchema);
    }
    Ok(())
}

#[cfg(test)]
#[path = "board_migration_tests.rs"]
mod tests;
