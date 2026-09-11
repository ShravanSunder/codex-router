//! Native migration ownership for the journal; legacy adoption never rewrites domain rows.
use communication_protocol::UuidIdentity;
use sqlx::{Connection, Row, SqliteConnection, migrate::MigrateError};

use crate::JournalError;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const BASELINE_VERSION: i64 = 202_609_110_001;
const CURRENT_SCHEMA_SQL: &str = include_str!("journal_schema.sql");
const BASELINE_SQL: &str = include_str!("../migrations/202609110001_journal_baseline.sql");

pub(crate) async fn initialize(
    connection: &mut SqliteConnection,
    new_identity: UuidIdentity,
) -> Result<UuidIdentity, JournalError> {
    let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
    let journal_id = initialize_transaction(
        &mut transaction,
        new_identity,
        &MIGRATOR,
        CURRENT_SCHEMA_SQL,
    )
    .await?;
    transaction.commit().await?;
    Ok(journal_id)
}

async fn initialize_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    new_identity: UuidIdentity,
    migrator: &sqlx::migrate::Migrator,
    current_schema: &str,
) -> Result<UuidIdentity, JournalError> {
    let has_history: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations')",
    )
    .fetch_one(&mut **transaction)
    .await?;
    let has_domain_objects: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations')",
    )
    .fetch_one(&mut **transaction)
    .await?;
    if !has_history && has_domain_objects {
        validate_schema(transaction, BASELINE_SQL).await?;
        read_identity(transaction).await?;
        migrator
            .run_direct(Some(BASELINE_VERSION), &mut **transaction, true)
            .await
            .map_err(map_migration_error)?;
    }
    migrator
        .run_direct(None, &mut **transaction, false)
        .await
        .map_err(map_migration_error)?;
    validate_schema(transaction, current_schema).await?;
    if !has_domain_objects {
        sqlx::query("INSERT INTO journal_checkpoint VALUES (1,0)")
            .execute(&mut **transaction)
            .await?;
        sqlx::query("INSERT INTO journal_metadata VALUES (1,1,?,0,0,0)")
            .bind(String::from(new_identity))
            .execute(&mut **transaction)
            .await?;
    }
    read_identity(transaction).await
}

async fn read_identity(connection: &mut SqliteConnection) -> Result<UuidIdentity, JournalError> {
    let row = sqlx::query("SELECT version,journal_id FROM journal_metadata WHERE singleton=1")
        .fetch_optional(&mut *connection)
        .await?
        .ok_or(JournalError::InvalidStorage)?;
    if row.try_get::<i64, _>("version")? != 1 {
        return Err(JournalError::InvalidStorage);
    }
    let checkpoint: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM journal_checkpoint WHERE singleton=1)")
            .fetch_one(connection)
            .await?;
    if !checkpoint {
        return Err(JournalError::InvalidStorage);
    }
    UuidIdentity::try_from(row.try_get::<String, _>("journal_id")?)
        .map_err(|_| JournalError::InvalidStorage)
}

// The legacy initializer emitted these exact definitions. Compare all objects,
// including constraints, so a version marker cannot bless an incompatible database.
// Future migrations must update the current target definition source here too.
async fn validate_schema(
    connection: &mut SqliteConnection,
    expected_schema: &str,
) -> Result<(), JournalError> {
    let rows = sqlx::query(
        "SELECT sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations' ORDER BY name",
    )
    .fetch_all(connection)
    .await?;
    let mut actual = rows
        .into_iter()
        .map(|row| {
            row.try_get::<String, _>("sql")
                .map(|sql| normalize_definition(&sql))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut expected = expected_schema
        .split(';')
        .filter(|statement| !statement.trim().is_empty())
        .map(normalize_definition)
        .collect::<Vec<_>>();
    actual.sort();
    expected.sort();
    if actual != expected {
        return Err(JournalError::InvalidStorage);
    }
    Ok(())
}

fn normalize_definition(sql: &str) -> String {
    sql.chars()
        .filter(|character| !character.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

#[allow(deprecated)]
fn map_migration_error(error: MigrateError) -> JournalError {
    match error {
        MigrateError::Execute(error) | MigrateError::ExecuteMigration(error, _) => {
            JournalError::Storage(error)
        }
        _ => JournalError::InvalidStorage,
    }
}

#[cfg(test)]
mod tests;
