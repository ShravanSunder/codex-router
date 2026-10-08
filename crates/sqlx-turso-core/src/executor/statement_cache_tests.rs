//! A cached prepared statement must return the current schema's result columns
//!
//! The engine reprepares a statement only on its first step, after the driver and the SDK have
//! already taken the column list, so a statement cached before a schema change would otherwise
//! keep returning the old shape.

use sqlx_core::{
    column::Column,
    connection::{ConnectOptions, Connection},
    executor::Executor,
    query::query,
    row::Row,
    sql_str::SqlStr,
};

use crate::{Turso, TursoConnectOptions, TursoConnection};

const SELECT_ALL: &str = "SELECT * FROM items";

async fn warmed_connection(create_table: &'static str) -> sqlx_core::Result<TursoConnection> {
    let mut connection = TursoConnectOptions::new().connect().await?;
    (&mut connection).execute(create_table).await?;
    (&mut connection)
        .execute("INSERT INTO items (id) VALUES (1)")
        .await?;
    // A persistent query prepares and caches the statement.
    query::<Turso>(SELECT_ALL)
        .fetch_one(&mut connection)
        .await?;
    Ok(connection)
}

fn column_names(row: &crate::TursoRow) -> Vec<String> {
    row.columns()
        .iter()
        .map(|column| column.name().to_owned())
        .collect()
}

#[tokio::test]
async fn a_cached_statement_returns_a_column_added_after_it_was_prepared() -> sqlx_core::Result<()>
{
    // Arrange
    let mut connection = warmed_connection("CREATE TABLE items (id INTEGER PRIMARY KEY)").await?;
    (&mut connection)
        .execute("ALTER TABLE items ADD COLUMN label TEXT")
        .await?;
    (&mut connection)
        .execute("UPDATE items SET label = 'added' WHERE id = 1")
        .await?;

    // Act
    let row = query::<Turso>(SELECT_ALL)
        .fetch_one(&mut connection)
        .await?;
    let describe = (&mut connection)
        .describe(SqlStr::from_static(SELECT_ALL))
        .await?;

    // Assert
    assert_eq!(column_names(&row), ["id", "label"]);
    assert_eq!(row.try_get::<String, _>("label")?, "added");
    assert_eq!(describe.columns().len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_cached_statement_returns_a_renamed_column_under_its_new_name() -> sqlx_core::Result<()> {
    // Arrange
    let mut connection =
        warmed_connection("CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT)").await?;
    (&mut connection)
        .execute("ALTER TABLE items RENAME COLUMN label TO title")
        .await?;

    // Act
    let row = query::<Turso>(SELECT_ALL)
        .fetch_one(&mut connection)
        .await?;

    // Assert
    assert_eq!(column_names(&row), ["id", "title"]);
    assert!(row.try_get_raw("label").is_err());
    Ok(())
}

#[tokio::test]
async fn an_unchanged_schema_keeps_the_cached_statements() -> sqlx_core::Result<()> {
    // Arrange
    let mut connection = warmed_connection("CREATE TABLE items (id INTEGER PRIMARY KEY)").await?;
    let cached = connection.cached_statements_size();

    // Act
    query::<Turso>(SELECT_ALL)
        .fetch_one(&mut connection)
        .await?;

    // Assert: nothing was evicted or re-prepared into a new entry
    assert!(cached > 0);
    assert_eq!(connection.cached_statements_size(), cached);
    Ok(())
}

#[tokio::test]
async fn a_cached_statement_sees_a_column_another_connection_added() -> sqlx_core::Result<()> {
    // Arrange: fetch_one stops the reader's cached statement after its first row
    let directory = tempfile::tempdir()?;
    let options = TursoConnectOptions::new()
        .filename(directory.path().join("shared.db"))
        .create_if_missing(true);
    let mut writer = options.connect().await?;
    let mut reader = options.connect().await?;
    (&mut writer)
        .execute("CREATE TABLE items (id INTEGER PRIMARY KEY)")
        .await?;
    (&mut writer)
        .execute("INSERT INTO items (id) VALUES (1), (2)")
        .await?;
    query::<Turso>(SELECT_ALL).fetch_one(&mut reader).await?;
    (&mut writer)
        .execute("ALTER TABLE items ADD COLUMN label TEXT")
        .await?;

    // Act
    let row = query::<Turso>(SELECT_ALL).fetch_one(&mut reader).await?;

    // Assert
    assert_eq!(column_names(&row), ["id", "label"]);
    Ok(())
}
