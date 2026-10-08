//! Real import, recovery, transaction and stale-cache scenarios against owned SQLite files.
use super::super::interaction_history_database::{
    HistoryStorageFailure, ImportTestFault, initialize_history, initialize_history_with_fault,
};
use super::*;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;

async fn observer(directory: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(directory.join("interaction.sqlite")),
    )
    .await
    .expect("independent SQLite connection")
}

async fn add_refusal(
    store: &InteractionHistoryStore,
    request_id: &str,
) -> Result<(), super::super::InteractionHistoryError> {
    let InteractionHistoryRecord::RefusedApproval {
        requester,
        approver,
        refusal,
    } = refused_approval(request_id)
    else {
        panic!("refusal fixture");
    };
    store
        .record_refused_approval(requester, approver, refusal)
        .await
}

async fn source_rows(directory: &Path, rows: &[&str]) -> Vec<u8> {
    let records: BTreeMap<_, _> = rows.iter().map(|id| (*id, refused_approval(id))).collect();
    let bytes = serde_json::to_vec_pretty(&records).expect("valid import rows");
    tokio::fs::write(directory.join("interaction-history.json"), &bytes)
        .await
        .expect("source");
    bytes
}

#[tokio::test]
async fn populated_import_constraint_failure_rolls_back_schema_rows_and_marker_then_retries_same_file()
 {
    let directory = tempfile::tempdir().expect("rollback fixture");
    let original = source_rows(directory.path(), &["first", "second"]).await;
    let source = directory.path().join("interaction-history.json");
    let database = directory.path().join("interaction.sqlite");
    let failed = initialize_history_with_fault(
        &database,
        &source,
        ImportTestFault::ConstraintFailureAfterRows,
    )
    .await;
    assert!(matches!(
        failed,
        Err(HistoryStorageFailure::StorageUnavailable)
    ));
    assert!(
        database.is_file(),
        "retry must use the original failed artifact"
    );
    let mut connection = observer(directory.path()).await;
    let objects: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
            .fetch_one(&mut connection)
            .await
            .expect("rollback schema inspection");
    assert_eq!(
        objects, 0,
        "schema, populated rows and migration history all roll back"
    );
    connection.close().await.expect("close observer");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("retry same artifact");
    assert_eq!(store.list_all().await.len(), 2);
    let timestamps = store.data.lock().await.created_at.clone();
    assert_eq!(timestamps["first"], timestamps["second"]);
    drop(store);
    let reopened = InteractionHistoryStore::load(source.clone())
        .await
        .expect("reopen committed import");
    assert_eq!(reopened.data.lock().await.created_at, timestamps);
    assert_eq!(
        tokio::fs::read(source).await.expect("original bytes"),
        original
    );
}

#[tokio::test]
async fn malformed_source_matrix_rejects_without_rewriting_or_committing_rows() {
    let valid = serde_json::to_value(refused_approval("record")).expect("fixture");
    let mut bad_timestamp = valid.clone();
    bad_timestamp["createdAt"] = "2026-10-01T00:00:00+00:00".into();
    let mut bad_reason = valid.clone();
    bad_reason["refusal"]["reason"] = "".into();
    let mut bad_identity = valid.clone();
    bad_identity["requester"]["sessionId"] = "".into();
    let mut bad_variant = valid.clone();
    bad_variant["kind"] = "unknownInteraction".into();
    let bad_answer = InteractionHistoryRecord::Question {
        requester: session_ref("requester"),
        approver: Identity::Session {
            session: session_ref("approver"),
        },
        request: question("record"),
        state: super::super::super::QuestionHistoryState::Answered {
            content: serde_json::from_value(serde_json::json!({"yes":"wrong-type"}))
                .expect("structurally valid answer with wrong domain type"),
        },
    };
    let serialized = serde_json::to_string(&valid).expect("duplicate nested fixture");
    let nested_duplicate = serialized.replacen(
        "\"requestId\":\"record\"",
        "\"requestId\":\"record\",\"requestId\":\"record\"",
        1,
    );
    assert_ne!(
        nested_duplicate, serialized,
        "fixture duplicates an owned nested key"
    );
    let inputs = [
        b"{".to_vec(),
        b"[]".to_vec(),
        serde_json::to_vec(&serde_json::json!({"wrong-id":valid})).expect("wrong key"),
        serde_json::to_vec(&serde_json::json!({"record":bad_timestamp})).expect("timestamp"),
        serde_json::to_vec(&serde_json::json!({"record":bad_reason})).expect("reason"),
        serde_json::to_vec(&serde_json::json!({"record":bad_identity})).expect("identity"),
        serde_json::to_vec(&serde_json::json!({"record":bad_variant})).expect("variant"),
        serde_json::to_vec(&serde_json::json!({"record":bad_answer})).expect("answer"),
        format!("{{\"record\":{nested_duplicate}}}").into_bytes(),
    ];
    for bytes in inputs {
        let directory = tempfile::tempdir().expect("invalid fixture");
        let source = directory.path().join("interaction-history.json");
        tokio::fs::write(&source, &bytes)
            .await
            .expect("invalid source");
        let result =
            initialize_history(&directory.path().join("interaction.sqlite"), &source).await;
        assert!(matches!(
            result,
            Err(HistoryStorageFailure::InvalidImportSource)
        ));
        assert_eq!(
            tokio::fs::read(source)
                .await
                .expect("invalid bytes retained"),
            bytes
        );
        let mut connection = observer(directory.path()).await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'")
                .fetch_one(&mut connection)
                .await
                .expect("no authoritative schema");
        assert_eq!(count, 0);
    }
}

#[tokio::test]
async fn owned_row_corruption_is_distinct_from_recovery_source_divergence() {
    let directory = tempfile::tempdir().expect("stored row fixture");
    let original = source_rows(directory.path(), &["record"]).await;
    let source = directory.path().join("interaction-history.json");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("import");
    drop(store);
    let mut connection = observer(directory.path()).await;
    sqlx::query("UPDATE typed_interaction_history SET record_json='{}'")
        .execute(&mut connection)
        .await
        .expect("corrupt owned row");
    connection.close().await.expect("close poison writer");
    let result = initialize_history(&directory.path().join("interaction.sqlite"), &source).await;
    assert!(matches!(
        result,
        Err(HistoryStorageFailure::InvalidStoredRecord)
    ));
    assert_eq!(
        tokio::fs::read(source).await.expect("source unchanged"),
        original
    );
}

#[tokio::test]
async fn stored_text_columns_reject_blob_coercion_even_when_payload_bytes_are_valid() {
    for (column, statement) in [
        (
            "request_id",
            "UPDATE typed_interaction_history SET request_id=CAST(request_id AS BLOB)",
        ),
        (
            "record_json",
            "UPDATE typed_interaction_history SET record_json=CAST(record_json AS BLOB)",
        ),
        (
            "created_at",
            "UPDATE typed_interaction_history SET created_at=CAST(created_at AS BLOB)",
        ),
    ] {
        let directory = tempfile::tempdir().expect("stored SQLite class fixture");
        let original = source_rows(directory.path(), &["record"]).await;
        let source = directory.path().join("interaction-history.json");
        let store = InteractionHistoryStore::load(source.clone())
            .await
            .expect("valid store");
        drop(store);
        let mut independent = observer(directory.path()).await;
        sqlx::query(statement)
            .execute(&mut independent)
            .await
            .expect("valid bytes in wrong storage class");
        independent.close().await.expect("close negative writer");
        assert!(
            matches!(
                initialize_history(&source.with_file_name("interaction.sqlite"), &source).await,
                Err(HistoryStorageFailure::InvalidStoredRecord)
            ),
            "column: {column}"
        );
        assert_eq!(
            tokio::fs::read(source).await.expect("original source"),
            original
        );
    }
}

#[tokio::test]
async fn owned_schema_and_metadata_negative_matrix_fails_closed() {
    let corruptions = [
        "UPDATE _sqlx_migrations SET success=0",
        "UPDATE _sqlx_migrations SET success=2",
        "UPDATE _sqlx_migrations SET success='1broken'",
        "UPDATE _sqlx_migrations SET checksum=x'00'",
        "UPDATE _sqlx_migrations SET version=version+1",
        "UPDATE _sqlx_migrations SET version=version+0.5",
        "UPDATE _sqlx_migrations SET checksum=CAST(checksum AS TEXT)",
        "DELETE FROM _sqlx_migrations",
        "UPDATE interaction_history_import SET revision=-1",
        "UPDATE interaction_history_import SET revision=1.5",
        "UPDATE interaction_history_import SET metadata_id=2",
        "UPDATE interaction_history_import SET source_sha256='invalid'",
        "DELETE FROM interaction_history_import",
        "INSERT INTO interaction_history_import VALUES (2,0,NULL,0)",
        "ALTER TABLE typed_interaction_history ADD COLUMN unexpected TEXT",
        "DROP TABLE typed_interaction_history",
        "CREATE TABLE unrelated (value TEXT)",
        "PRAGMA user_version=7",
        "PRAGMA application_id=9",
    ];
    for statement in corruptions {
        let directory = tempfile::tempdir().expect("owned negative fixture");
        let source = directory.path().join("interaction-history.json");
        let store = InteractionHistoryStore::load(source.clone())
            .await
            .expect("empty owned database");
        drop(store);
        let mut connection = observer(directory.path()).await;
        sqlx::query(statement)
            .execute(&mut connection)
            .await
            .expect("negative mutation");
        connection.close().await.expect("close invalid writer");
        let result =
            initialize_history(&directory.path().join("interaction.sqlite"), &source).await;
        assert!(
            matches!(result, Err(HistoryStorageFailure::InvalidSchema)),
            "negative: {statement}"
        );
    }
}

#[tokio::test]
async fn corrupt_database_bytes_are_preserved_and_rejected() {
    let directory = tempfile::tempdir().expect("corrupt file fixture");
    let database = directory.path().join("interaction.sqlite");
    let bytes = b"not a SQLite database";
    tokio::fs::write(&database, bytes)
        .await
        .expect("corrupt database");
    let result = initialize_history(
        &database,
        &directory.path().join("interaction-history.json"),
    )
    .await;
    assert!(matches!(result, Err(HistoryStorageFailure::InvalidSchema)));
    assert_eq!(
        tokio::fs::read(database).await.expect("preserved database"),
        bytes
    );
}

#[tokio::test]
async fn source_presence_and_digest_changes_reject_then_exact_restoration_reopens_new_writes() {
    let directory = tempfile::tempdir().expect("source provenance fixture");
    let original = source_rows(directory.path(), &["imported"]).await;
    let source = directory.path().join("interaction-history.json");
    let database = directory.path().join("interaction.sqlite");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("import");
    add_refusal(&store, "sqlite-only")
        .await
        .expect("new SQLite write");
    drop(store);
    tokio::fs::write(&source, b"{}")
        .await
        .expect("old writer divergence");
    assert!(matches!(
        initialize_history(&database, &source).await,
        Err(HistoryStorageFailure::RecoverySourceChanged)
    ));
    tokio::fs::rename(&source, directory.path().join("divergent-source.json"))
        .await
        .expect("preserve divergence");
    assert!(matches!(
        initialize_history(&database, &source).await,
        Err(HistoryStorageFailure::RecoverySourceChanged)
    ));
    tokio::fs::write(&source, &original)
        .await
        .expect("restore exact original");
    let reopened = InteractionHistoryStore::load(source)
        .await
        .expect("restored provenance");
    assert_eq!(
        reopened.list_all().await.len(),
        2,
        "restoration never reimports over newer SQLite history"
    );
    assert_eq!(
        tokio::fs::read(directory.path().join("divergent-source.json"))
            .await
            .expect("divergence retained"),
        b"{}"
    );

    let absent_directory = tempfile::tempdir().expect("absent source fixture");
    let source = absent_directory.path().join("interaction-history.json");
    let empty = InteractionHistoryStore::load(source.clone())
        .await
        .expect("absent-source initialization");
    drop(empty);
    tokio::fs::write(&source, b"{}")
        .await
        .expect("newly appeared source");
    assert!(matches!(
        initialize_history(&source.with_file_name("interaction.sqlite"), &source).await,
        Err(HistoryStorageFailure::RecoverySourceChanged)
    ));
    tokio::fs::rename(
        &source,
        absent_directory.path().join("unexpected-source.json"),
    )
    .await
    .expect("restore expected absence without deletion");
    assert!(InteractionHistoryStore::load(source).await.is_ok());
}

fn question(request_id: &str) -> session_event_model::QuestionRequest {
    serde_json::from_value(
        serde_json::json!({"requestId":request_id,"prompt":"Proceed?","fields":[
            {"kind":"boolean","fieldId":"yes","label":"Yes","description":null,"required":true}
        ]}),
    )
    .expect("question")
}

#[path = "interaction_history_sqlite_lifecycle_tests.rs"]
mod lifecycle_scenarios;
