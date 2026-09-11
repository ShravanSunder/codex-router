use communication_protocol::UuidIdentity;
use lifecycle_observation::{JournalError, ObservationJournal};
use sqlx::{Connection, SqliteConnection};

const LEGACY_SCHEMA: &str = include_str!("fixtures/journal_v1.sql");

fn identity() -> Result<UuidIdentity, Box<dyn std::error::Error>> {
    Ok("00000000-0000-4000-8000-000000000001"
        .to_owned()
        .try_into()?)
}

async fn connect(path: &std::path::Path) -> Result<SqliteConnection, sqlx::Error> {
    SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true),
    )
    .await
}

#[tokio::test]
async fn fresh_journal_registers_native_history_and_reopens() {
    let path = std::env::temp_dir().join(format!(
        "journal-native-fresh-{}.sqlite",
        std::process::id()
    ));
    let journal = ObservationJournal::open(&path, identity().unwrap())
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    journal.close().await;
    let mut connection = connect(&path).await.unwrap();
    let history: (i64, bool) = sqlx::query_as("SELECT version,success FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    assert_eq!(history, (202609110001, true));
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    ObservationJournal::open(&path, identity().unwrap())
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"))
        .close()
        .await;
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("journal migration proof: {error}"));
}

#[tokio::test]
async fn legacy_adoption_preserves_all_rows_and_rejects_changed_history() {
    let path = std::env::temp_dir().join(format!(
        "journal-native-adopt-{}.sqlite",
        std::process::id()
    ));
    let mut connection = connect(&path).await.unwrap();
    sqlx::raw_sql(LEGACY_SCHEMA)
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    sqlx::raw_sql("INSERT INTO journal_metadata VALUES (1,1,'00000000-0000-4000-8000-000000000001',9,100,24); INSERT INTO journal_checkpoint VALUES(1,7); INSERT INTO lifecycle_records VALUES(9,100,'retained record'); INSERT INTO thread_addresses VALUES('address','retained address',16); INSERT INTO checkpoint_addresses VALUES('checkpoint','retained checkpoint');").execute(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    let before = snapshot(&mut connection).await.unwrap();
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    let journal = ObservationJournal::open(&path, identity().unwrap())
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    assert_eq!(journal.journal_id(), &identity().unwrap());
    journal.close().await;
    let mut connection = connect(&path).await.unwrap();
    assert_eq!(snapshot(&mut connection).await.unwrap(), before);
    sqlx::query("UPDATE _sqlx_migrations SET checksum=x'00'")
        .execute(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    assert!(matches!(
        ObservationJournal::open(&path, identity().unwrap()).await,
        Err(JournalError::InvalidStorage)
    ));
    let mut connection = connect(&path).await.unwrap();
    assert_eq!(snapshot(&mut connection).await.unwrap(), before);
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("journal migration proof: {error}"));
}

async fn snapshot(connection: &mut SqliteConnection) -> Result<Vec<String>, sqlx::Error> {
    let mut result = Vec::new();
    for query in [
        "SELECT json_array(singleton,version,journal_id,last_sequence,retention_clock,payload_bytes) FROM journal_metadata ORDER BY singleton",
        "SELECT json_array(sequence,retention_at,observation_json) FROM lifecycle_records ORDER BY sequence",
        "SELECT json_array(address_key,entry_json,payload_bytes) FROM thread_addresses ORDER BY address_key",
        "SELECT json_array(singleton,sequence) FROM journal_checkpoint ORDER BY singleton",
        "SELECT json_array(address_key,entry_json) FROM checkpoint_addresses ORDER BY address_key",
    ] {
        result.extend(
            sqlx::query_scalar::<_, String>(query)
                .fetch_all(&mut *connection)
                .await?,
        );
    }
    Ok(result)
}

#[tokio::test]
async fn incompatible_legacy_schema_or_metadata_is_rejected_without_history() {
    for (index, change) in [
        "ALTER TABLE lifecycle_records ADD COLUMN unexpected TEXT",
        "DROP TABLE checkpoint_addresses",
        "UPDATE journal_metadata SET version=2",
        "UPDATE journal_metadata SET journal_id='invalid'",
        "DELETE FROM journal_checkpoint",
        "CREATE TRIGGER unexpected AFTER INSERT ON lifecycle_records BEGIN DELETE FROM thread_addresses; END",
    ].into_iter().enumerate() {
        let path = std::env::temp_dir().join(format!("journal-native-invalid-{}-{index}.sqlite", std::process::id()));
        let mut connection = connect(&path).await.unwrap();
        sqlx::raw_sql(LEGACY_SCHEMA).execute(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        sqlx::raw_sql("INSERT INTO journal_metadata VALUES(1,1,'00000000-0000-4000-8000-000000000001',0,0,0); INSERT INTO journal_checkpoint VALUES(1,0);").execute(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        sqlx::raw_sql(change).execute(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        let before: Vec<(String, Option<String>)> = sqlx::query_as("SELECT name,sql FROM sqlite_master ORDER BY name").fetch_all(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        connection.close().await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        assert!(matches!(ObservationJournal::open(&path, identity().unwrap()).await, Err(JournalError::InvalidStorage)), "case {index}");
        let mut connection = connect(&path).await.unwrap();
        let after: Vec<(String, Option<String>)> = sqlx::query_as("SELECT name,sql FROM sqlite_master ORDER BY name").fetch_all(&mut connection).await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        assert_eq!(after, before, "case {index}: adoption must not write schema/history");
        connection.close().await.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        std::fs::remove_file(path).unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    }
}

#[tokio::test]
async fn dirty_or_unknown_native_history_is_rejected_without_changes() {
    for (index, change) in [
        "UPDATE _sqlx_migrations SET success=FALSE",
        "UPDATE _sqlx_migrations SET version=999999999999",
    ]
    .into_iter()
    .enumerate()
    {
        let path = std::env::temp_dir().join(format!(
            "journal-native-history-{}-{index}.sqlite",
            std::process::id()
        ));
        ObservationJournal::open(&path, identity().unwrap())
            .await
            .unwrap_or_else(|error| panic!("journal migration proof: {error}"))
            .close()
            .await;
        let mut connection = connect(&path).await.unwrap();
        let before = snapshot(&mut connection).await.unwrap();
        sqlx::query(change)
            .execute(&mut connection)
            .await
            .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        let history: Vec<(i64, bool, Vec<u8>)> =
            sqlx::query_as("SELECT version,success,checksum FROM _sqlx_migrations")
                .fetch_all(&mut connection)
                .await
                .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        connection
            .close()
            .await
            .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        assert!(matches!(
            ObservationJournal::open(&path, identity().unwrap()).await,
            Err(JournalError::InvalidStorage)
        ));
        let mut connection = connect(&path).await.unwrap();
        assert_eq!(snapshot(&mut connection).await.unwrap(), before);
        let after: Vec<(i64, bool, Vec<u8>)> =
            sqlx::query_as("SELECT version,success,checksum FROM _sqlx_migrations")
                .fetch_all(&mut connection)
                .await
                .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        assert_eq!(history, after);
        connection
            .close()
            .await
            .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
        std::fs::remove_file(path)
            .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    }
}

#[tokio::test]
async fn concurrent_open_registers_one_baseline_and_preserves_one_identity() {
    let path = std::env::temp_dir().join(format!(
        "journal-native-concurrent-{}.sqlite",
        std::process::id()
    ));
    let other: UuidIdentity = "00000000-0000-4000-8000-000000000002"
        .to_owned()
        .try_into()
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    let (first, second) = tokio::join!(
        ObservationJournal::open(&path, identity().unwrap()),
        ObservationJournal::open(&path, other)
    );
    let first = first.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    let second = second.unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    assert_eq!(first.journal_id(), second.journal_id());
    first.close().await;
    second.close().await;
    let mut connection = connect(&path).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success=TRUE")
        .fetch_one(&mut connection)
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    assert_eq!(count, 1);
    connection
        .close()
        .await
        .unwrap_or_else(|error| panic!("journal migration proof: {error}"));
    std::fs::remove_file(path).unwrap_or_else(|error| panic!("journal migration proof: {error}"));
}
