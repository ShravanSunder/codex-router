//! Atomic schema creation uses SQLite user_version, not another domain table.
use crate::StorageError;
use sqlx::{Connection, SqliteConnection};

pub(crate) async fn initialize(connection: &mut SqliteConnection) -> Result<(), StorageError> {
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut *transaction)
        .await?;
    match version {
        0 => {
            let existing: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut *transaction).await?;
            if existing != 0 {
                return Err(StorageError::InvalidSchema);
            }
            sqlx::raw_sql(include_str!("automation_schema.sql"))
                .execute(&mut *transaction)
                .await?;
            sqlx::query("PRAGMA user_version=1")
                .execute(&mut *transaction)
                .await?;
        }
        1 => {
            let names: Vec<String> = sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .fetch_all(&mut *transaction).await?;
            let expected = [
                "automation_events",
                "instruction_documents",
                "instruction_revisions",
                "mailbox_deliveries",
                "operation_receipts",
                "schedule_definitions",
                "schedule_timing_state",
                "thread_bindings",
                "wakeup_definitions",
                "workflow_runs",
            ];
            if names.iter().map(String::as_str).ne(expected) {
                return Err(StorageError::InvalidSchema);
            }
        }
        other => return Err(StorageError::UnsupportedVersion(other)),
    }
    transaction.commit().await?;
    Ok(())
}
