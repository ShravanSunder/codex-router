use codex_acp_adapter::{AcpStoredSessions, NativeStoredSessions};
use serde_json::json;
use sqlx::{Connection, Executor, sqlite::SqliteConnectOptions};

#[tokio::test]
async fn catalog_pages_preserve_scope_and_long_directory_without_writing_history() {
    // Arrange: an owned database fixture, never the user's Codex history.
    let root = std::env::temp_dir().join(format!(
        "acp-stored-catalog-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let database = root.join("state_5.sqlite");
    let mut connection = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database)
            .create_if_missing(true),
    )
    .await
    .unwrap();
    connection.execute("CREATE TABLE threads (id TEXT PRIMARY KEY, rollout_path TEXT, cwd TEXT, model_provider TEXT, model TEXT, source TEXT, thread_source TEXT, git_branch TEXT, git_origin_url TEXT, name TEXT, title TEXT, preview TEXT, first_user_message TEXT, created_at_ms INTEGER, updated_at_ms INTEGER, recency_at_ms INTEGER, archived INTEGER NOT NULL)").await.unwrap();
    connection
        .execute("CREATE INDEX idx_threads_updated_at_ms ON threads(updated_at_ms DESC, id DESC)")
        .await
        .unwrap();
    let directory = format!("/{}", "segment/".repeat(101));
    for index in 0..101 {
        sqlx::query(
            "INSERT INTO threads (id, cwd, title, updated_at_ms, archived) VALUES (?, ?, ?, ?, 0)",
        )
        .bind(format!("thread-{index:03}"))
        .bind(&directory)
        .bind(format!("Session {index}"))
        .bind(i64::from(index))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();
    let original = std::fs::read(&database).unwrap();
    let catalog = NativeStoredSessions::new(root.clone(), "endpoint-one".to_owned());

    // Act: continue a page with the same long cwd, then try other scopes.
    let first = catalog.list(json!({"cwd": directory})).await.unwrap();
    let cursor = first["nextCursor"].as_str().unwrap();
    let second = catalog
        .list(json!({"cwd": directory, "cursor": cursor}))
        .await
        .unwrap();
    let wrong_directory = catalog
        .list(json!({"cwd": "/elsewhere", "cursor": cursor}))
        .await;
    let other = NativeStoredSessions::new(root.clone(), "endpoint-two".to_owned());
    let wrong_endpoint = other
        .list(json!({"cwd": directory, "cursor": cursor}))
        .await;

    // Assert: exact keyset continuation, scoped cursor, no history mutation.
    assert_eq!(first["sessions"].as_array().unwrap().len(), 100);
    assert_eq!(first["sessions"][0]["sessionId"], "thread-100");
    assert_eq!(second["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(second["sessions"][0]["sessionId"], "thread-000");
    assert!(second["nextCursor"].is_null());
    assert!(cursor.len() <= 1024);
    assert_eq!(
        wrong_directory.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(
        wrong_endpoint.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(std::fs::read(&database).unwrap(), original);
    std::fs::remove_file(database).unwrap();
    std::fs::remove_dir(root).unwrap();
}
