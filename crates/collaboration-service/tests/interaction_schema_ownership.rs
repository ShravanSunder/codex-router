use collaboration_protocol::EndpointRef;
use collaboration_service::{NativeControlBackend, NativeGenerationGate, ServiceInteractionBroker};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;

async fn load_broker(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let endpoint: EndpointRef = serde_json::from_value(serde_json::json!({
        "serviceId":"00000000-0000-4000-8000-000000000001", "endpointId":"claude-local"
    }))?;
    ServiceInteractionBroker::load(
        endpoint.service_id.clone(),
        NativeControlBackend {
            endpoint,
            gate: NativeGenerationGate::default(),
            codex_home: root.to_owned(),
        },
        root.join("approval-routes.json"),
    )
    .await
    .map(drop)
    .map_err(Into::into)
}

#[tokio::test]
async fn nonreserved_sqlite_prefix_table_blocks_foreign_adoption_and_owned_reopen() {
    for initialized in [false, true] {
        let directory = tempfile::tempdir().expect("isolated schema ownership fixture");
        if initialized {
            load_broker(directory.path())
                .await
                .expect("owned initialization");
        }
        let database = directory.path().join("interaction.sqlite");
        let mut foreign = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&database)
                .create_if_missing(true),
        )
        .await
        .expect("independent schema writer");
        // SQLite reserves sqlite_, but this name is a valid, unrelated user table.
        sqlx::query("CREATE TABLE sqliteXforeign (body TEXT NOT NULL)")
            .execute(&mut foreign)
            .await
            .expect("nonreserved table");
        sqlx::query("INSERT INTO sqliteXforeign VALUES ('preserve foreign content')")
            .execute(&mut foreign)
            .await
            .expect("foreign row");
        foreign.close().await.expect("close schema writer");
        let original = tokio::fs::read(&database)
            .await
            .expect("preinspection bytes");

        assert!(
            load_broker(directory.path()).await.is_err(),
            "nonreserved user table must reject initialization/reopen; initialized={initialized}"
        );
        assert_eq!(
            tokio::fs::read(&database)
                .await
                .expect("preserved database"),
            original
        );
        let mut observer = SqliteConnection::connect_with(
            &SqliteConnectOptions::new()
                .filename(&database)
                .read_only(true),
        )
        .await
        .expect("read-only foreign observer");
        let content: String = sqlx::query_scalar("SELECT body FROM sqliteXforeign")
            .fetch_one(&mut observer)
            .await
            .expect("foreign content survives");
        assert_eq!(content, "preserve foreign content");
        observer.close().await.expect("close observer");
    }
}
