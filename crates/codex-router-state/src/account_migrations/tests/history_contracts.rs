use super::*;

#[tokio::test]
async fn checksum_mismatch_rejects_without_repair() {
    let temporary_database = TemporaryDatabase::new("checksum_mismatch");
    let store = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("native database should initialize");
    store.close().await.expect("native store should close");
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00'")
        .execute(&mut connection)
        .await
        .expect("checksum should be corrupted");
    connection.close().await.expect("history should close");

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("checksum mismatch must reject");
    assert!(format!("{error}").contains("incompatible account migration history"));
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let checksum: Vec<u8> = sqlx::query_scalar("SELECT checksum FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .expect("checksum should query");
    assert_eq!(checksum, vec![0]);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn dirty_native_history_rejects_without_repair() {
    let temporary_database = TemporaryDatabase::new("dirty_history");
    let store = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect("native database should initialize");
    store.close().await.expect("native store should close");
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    sqlx::query("UPDATE _sqlx_migrations SET success = 0")
        .execute(&mut connection)
        .await
        .expect("history should become dirty");
    connection.close().await.expect("history should close");

    let error = AsyncSqliteStateStore::open(temporary_database.path())
        .await
        .expect_err("dirty history must reject");
    assert!(format!("{error}").contains("dirty account migration history"));
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let success: bool = sqlx::query_scalar("SELECT success FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .expect("dirty marker should query");
    assert!(!success);
    connection.close().await.expect("inspection should close");
}

#[tokio::test]
async fn legacy_and_native_read_only_openers_never_create_history() {
    let legacy_database = TemporaryDatabase::new("legacy_read_only");
    create_legacy_v13_database(legacy_database.path(), "legacy-read-only").await;
    let legacy_reader = AsyncSqliteStateStore::open_read_only(legacy_database.path())
        .await
        .expect("legacy v13 should remain readable");
    legacy_reader
        .close()
        .await
        .expect("legacy reader should close");
    assert_legacy_database_unchanged(legacy_database.path(), "legacy-read-only").await;

    let native_database = TemporaryDatabase::new("native_read_only");
    let native_writer = AsyncSqliteStateStore::open(native_database.path())
        .await
        .expect("native database should initialize");
    native_writer
        .close()
        .await
        .expect("native writer should close");
    let before = migration_history_bytes(native_database.path()).await;
    let native_reader = AsyncSqliteStateStore::open_read_only(native_database.path())
        .await
        .expect("native current database should read");
    native_reader
        .close()
        .await
        .expect("native reader should close");
    assert_eq!(
        migration_history_bytes(native_database.path()).await,
        before
    );
}

#[tokio::test]
async fn unsupported_async_legacy_version_preserves_rows_and_history_absence() {
    let temporary_database = TemporaryDatabase::new("unsupported_version");
    let mut connection = open_test_connection(temporary_database.path(), true).await;
    sqlx::raw_sql(
        "CREATE TABLE sentinel (value TEXT NOT NULL);
             INSERT INTO sentinel VALUES ('preserved');
             PRAGMA user_version = 6;",
    )
    .execute(&mut connection)
    .await
    .expect("unsupported fixture should initialize");
    connection.close().await.expect("fixture should close");

    assert_eq!(
        AsyncSqliteStateStore::open(temporary_database.path())
            .await
            .expect_err("unsupported async legacy version must reject"),
        crate::sqlite::StateStoreError::UnsupportedSchemaVersion { version: 6 }
    );
    let mut connection = open_test_connection(temporary_database.path(), false).await;
    let value: String = sqlx::query_scalar("SELECT value FROM sentinel")
        .fetch_one(&mut connection)
        .await
        .expect("sentinel should remain");
    assert_eq!(value, "preserved");
    let history_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations')",
        )
        .fetch_one(&mut connection)
        .await
        .expect("history presence should query");
    assert!(!history_exists);
    connection.close().await.expect("inspection should close");
}
