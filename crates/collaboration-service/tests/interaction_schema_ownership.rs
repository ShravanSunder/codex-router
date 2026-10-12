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

#[tokio::test]
async fn typed_json_is_ignored_on_fresh_startup_and_reopen() {
    // Structurally and semantically valid old typed history must not be imported.
    let original = br#"{"old-refusal":{"kind":"refusedApproval","requester":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"claude-local"},"sessionId":"old-requester"},"approver":{"kind":"human","humanId":"owner"},"refusal":{"requestId":"old-refusal","title":"Old refusal","description":null,"subject":null,"options":[],"reason":"fixture"}}}"#;
    // Validate the fixture independently so startup errors cannot manufacture a pass.
    let records: std::collections::BTreeMap<
        String,
        collaboration_service::InteractionHistoryRecord,
    > = serde_json::from_slice(original).expect("valid old typed JSON");
    assert_eq!(records["old-refusal"].request_id(), "old-refusal");
    let directory = tempfile::tempdir().expect("isolated hard cutover fixture");
    let json_path = directory.path().join("interaction-history.json");
    tokio::fs::write(&json_path, original)
        .await
        .expect("old JSON");
    load_broker(directory.path())
        .await
        .expect("fresh SQLite startup");
    let database = directory.path().join("interaction.sqlite");
    let mut observer = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&database)
            .read_only(true),
    )
    .await
    .expect("independent observer");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM typed_interaction_history")
        .fetch_one(&mut observer)
        .await
        .expect("actual empty SQLite history");
    assert_eq!(count, 0, "old typed JSON must not populate SQLite");
    observer.close().await.expect("close observer");
    assert_eq!(
        tokio::fs::read(&json_path).await.expect("untouched JSON"),
        original
    );
    tokio::fs::write(&json_path, b"malformed old history")
        .await
        .expect("changed old JSON");
    load_broker(directory.path())
        .await
        .expect("malformed JSON cannot block reopen");
    assert_eq!(
        tokio::fs::read(&json_path)
            .await
            .expect("untouched malformed JSON"),
        b"malformed old history"
    );
    tokio::fs::rename(
        &json_path,
        directory.path().join("retained-old-history.json"),
    )
    .await
    .expect("preserve old file");
    tokio::fs::create_dir(&json_path)
        .await
        .expect("unreadable-as-file old path");
    load_broker(directory.path())
        .await
        .expect("non-file JSON path cannot block reopen");
    assert!(json_path.is_dir());
    let fresh = tempfile::tempdir().expect("fresh malformed fixture");
    tokio::fs::create_dir(fresh.path().join("interaction-history.json"))
        .await
        .expect("non-file JSON path before first initialization");
    load_broker(fresh.path())
        .await
        .expect("non-file JSON path cannot block initialization");
}
