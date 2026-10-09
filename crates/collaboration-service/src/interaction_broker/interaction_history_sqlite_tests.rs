//! Real corruption, transaction and stale-cache scenarios against owned SQLite files.
use super::super::interaction_history_database::{HistoryStorageFailure, initialize_history};
use super::*;
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use std::path::Path;

pub(super) async fn observer(directory: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new().filename(directory.join("interaction.sqlite")),
    )
    .await
    .expect("independent SQLite connection")
}

pub(super) async fn add_refusal(
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

async fn seed_rows(directory: &Path, rows: &[&str]) {
    let store = InteractionHistoryStore::load(directory.join("interaction.sqlite"))
        .await
        .expect("fresh owned SQLite");
    for request_id in rows {
        add_refusal(&store, request_id)
            .await
            .expect("real row creation");
    }
}

#[tokio::test]
async fn malformed_stored_record_matrix_rejects_without_rewriting_rows() {
    let valid = serde_json::to_value(refused_approval("record")).expect("fixture");
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
                .expect("structural answer"),
        },
    };
    let serialized = serde_json::to_string(&valid).expect("record");
    let duplicate = serialized.replacen(
        "\"requestId\":\"record\"",
        "\"requestId\":\"record\",\"requestId\":\"record\"",
        1,
    );
    assert_ne!(duplicate, serialized);
    let timestamp = "2026-10-01T00:00:00.000000000Z";
    let inputs = [
        ("record", "{".to_owned(), timestamp),
        ("record", "[]".to_owned(), timestamp),
        ("wrong-id", serialized.clone(), timestamp),
        ("record", serialized, "2026-10-01T00:00:00+00:00"),
        (
            "record",
            serde_json::to_string(&bad_reason).expect("reason"),
            timestamp,
        ),
        (
            "record",
            serde_json::to_string(&bad_identity).expect("identity"),
            timestamp,
        ),
        (
            "record",
            serde_json::to_string(&bad_variant).expect("variant"),
            timestamp,
        ),
        (
            "record",
            serde_json::to_string(&bad_answer).expect("answer"),
            timestamp,
        ),
        ("record", duplicate, timestamp),
    ];
    for (request_id, record_json, created_at) in inputs {
        let directory = tempfile::tempdir().expect("invalid stored row fixture");
        seed_rows(directory.path(), &["record"]).await;
        let mut independent = observer(directory.path()).await;
        sqlx::query(
            "UPDATE typed_interaction_history SET request_id=?, record_json=?, created_at=?",
        )
        .bind(request_id)
        .bind(&record_json)
        .bind(created_at)
        .execute(&mut independent)
        .await
        .expect("corrupt owned row");
        independent.close().await.expect("close corruption writer");
        assert!(matches!(
            initialize_history(&directory.path().join("interaction.sqlite")).await,
            Err(HistoryStorageFailure::InvalidStoredRecord)
        ));
        let mut independent = observer(directory.path()).await;
        let unchanged: (String, String, String) = sqlx::query_as(
            "SELECT request_id,record_json,created_at FROM typed_interaction_history",
        )
        .fetch_one(&mut independent)
        .await
        .expect("preserved invalid row");
        assert_eq!(
            unchanged,
            (request_id.to_owned(), record_json, created_at.to_owned())
        );
    }
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
        seed_rows(directory.path(), &["record"]).await;
        let source = directory.path().join("interaction.sqlite");
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
                initialize_history(&source).await,
                Err(HistoryStorageFailure::InvalidStoredRecord)
            ),
            "column: {column}"
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
        "UPDATE interaction_history_revision SET revision=-1",
        "UPDATE interaction_history_revision SET revision=1.5",
        "UPDATE interaction_history_revision SET metadata_id=2",
        "DELETE FROM interaction_history_revision",
        "INSERT INTO interaction_history_revision VALUES (2,0)",
        "ALTER TABLE typed_interaction_history ADD COLUMN unexpected TEXT",
        "DROP TABLE typed_interaction_history",
        "CREATE TABLE unrelated (value TEXT)",
        "PRAGMA user_version=7",
        "PRAGMA application_id=9",
    ];
    for statement in corruptions {
        let directory = tempfile::tempdir().expect("owned negative fixture");
        let source = directory.path().join("interaction.sqlite");
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
        let result = initialize_history(&source).await;
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
    let result = initialize_history(&database).await;
    assert!(matches!(result, Err(HistoryStorageFailure::InvalidSchema)));
    assert_eq!(
        tokio::fs::read(database).await.expect("preserved database"),
        bytes
    );
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
