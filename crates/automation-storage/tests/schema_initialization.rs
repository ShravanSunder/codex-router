use automation_storage::AutomationStore;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn initializes_all_owned_tables_and_reopens_without_erasing_data()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: independent file inside OS temp; normal Codex/router state is never used.
    let path =
        std::env::temp_dir().join(format!("automation-schema-{}.sqlite", uuid::Uuid::now_v7()));
    // Act: initialize through the production storage entrypoint.
    AutomationStore::open(&path).await?.close().await?;
    let mut connection =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(&mut connection)
    .await?;
    // Assert: configuration, timing, Run and other owned persistence all exist.
    if count != 10 {
        return Err(format!("expected 10 domain tables, got {count}").into());
    }
    sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES ('test-event','schedule','test-schedule','changed','{}',1)").execute(&mut connection).await?;
    connection.close().await?;
    AutomationStore::open(&path).await?.close().await?;
    let mut reopened =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM automation_events")
        .fetch_one(&mut reopened)
        .await?;
    if retained != 1 {
        return Err("reopen erased recorded events".into());
    }
    let triggers: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE type='trigger'")
            .fetch_one(&mut reopened)
            .await?;
    if triggers != 0 {
        return Err("unexpected SQL triggers".into());
    }
    reopened.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
