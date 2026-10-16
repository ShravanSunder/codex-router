use super::super::{InteractionHistoryRecord, RefusedApprovalOption, RefusedTypedApproval};
use super::InteractionHistoryStore;
use chrono::{Duration, Utc};
use message_board::{Identity, SessionRef};

#[path = "interaction_history_sqlite_tests.rs"]
mod sqlite_scenarios;

fn session_ref(session_id: &str) -> SessionRef {
    serde_json::from_value(serde_json::json!({
        "endpoint":{
            "serviceId":"018f47d2-24d5-7a68-b9ec-6f759c39458f",
            "endpointId":"codex-local"
        },
        "sessionId":session_id
    }))
    .expect("valid fixture session")
}

fn refused_approval(request_id: &str) -> InteractionHistoryRecord {
    let requester = session_ref("requester");
    InteractionHistoryRecord::RefusedApproval {
        requester: requester.clone(),
        approver: Identity::Session { session: requester },
        refusal: RefusedTypedApproval {
            request_id: request_id.to_owned(),
            title: "Rejected approval".to_owned(),
            description: None,
            subject: None,
            options: vec![RefusedApprovalOption {
                option_id: "reject".to_owned(),
                label: "Reject".to_owned(),
                provider_kind: "fixture".to_owned(),
            }],
            reason: "fixture refusal".to_owned(),
        },
    }
}

#[tokio::test]
async fn sqlite_creation_reopens_records_without_touching_old_json() {
    let directory = tempfile::tempdir().expect("isolated SQLite fixture");
    let source = directory.path().join("interaction-history.json");
    let original = b"malformed obsolete JSON";
    tokio::fs::write(&source, original)
        .await
        .expect("obsolete file");
    let database = directory.path().join("interaction.sqlite");
    let store = InteractionHistoryStore::load(database.clone())
        .await
        .expect("fresh store");
    assert!(store.list_all().await.is_empty());
    sqlite_scenarios::add_refusal(&store, "new-refusal")
        .await
        .expect("real mutation");
    let first_stamp = store.data.lock().await.created_at["new-refusal"];
    drop(store);
    tokio::fs::write(&source, b"changed obsolete file")
        .await
        .expect("old writer");
    let reopened = InteractionHistoryStore::load(database)
        .await
        .expect("reopen");
    assert!(reopened.interaction("new-refusal").await.is_some());
    assert_eq!(
        reopened.data.lock().await.created_at["new-refusal"],
        first_stamp
    );
    assert_eq!(
        tokio::fs::read(source).await.expect("untouched old file"),
        b"changed obsolete file"
    );
    let mut observer = sqlite_scenarios::observer(directory.path()).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM typed_interaction_history")
        .fetch_one(&mut observer)
        .await
        .expect("durable rows");
    assert_eq!(count, 1);
}

#[tokio::test]
async fn unrelated_database_is_rejected_without_adopting_its_schema() {
    let directory = tempfile::tempdir().expect("isolated foreign database");
    let database = directory.path().join("interaction.sqlite");
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true);
    let mut foreign = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&options)
        .await
        .expect("foreign connection");
    sqlx::query("CREATE TABLE foreign_records (body TEXT NOT NULL)")
        .execute(&mut foreign)
        .await
        .expect("foreign schema");
    sqlx::query("INSERT INTO foreign_records VALUES ('keep this record')")
        .execute(&mut foreign)
        .await
        .expect("foreign record");
    sqlx::Connection::close(foreign)
        .await
        .expect("close foreign database");
    let original = tokio::fs::read(&database).await.expect("foreign bytes");
    assert!(
        InteractionHistoryStore::load(directory.path().join("interaction.sqlite"))
            .await
            .is_err(),
        "an unknown database must never be adopted"
    );
    assert_eq!(
        tokio::fs::read(database)
            .await
            .expect("foreign bytes retained"),
        original
    );
}

#[tokio::test]
async fn new_records_keep_creation_stamp_and_prune_strictly_after_thirty_days() {
    let directory = tempfile::tempdir().expect("isolated retention fixture");
    let path = directory.path().join("interaction.sqlite");
    let store = InteractionHistoryStore::load(path.clone())
        .await
        .expect("fresh store");
    let started_at = Utc::now();
    sqlite_scenarios::add_refusal(&store, "new-request")
        .await
        .expect("record");
    let created_at = store.data.lock().await.created_at["new-request"];
    assert!(created_at >= started_at && created_at <= Utc::now());
    drop(store);
    let store = InteractionHistoryStore::load(path).await.expect("reopen");
    assert_eq!(
        store.data.lock().await.created_at["new-request"],
        created_at
    );
    let exact_cutoff = created_at + Duration::days(30);
    assert_eq!(
        store
            .prune_expired(exact_cutoff, 500)
            .await
            .expect("exact cutoff"),
        0
    );
    assert_eq!(
        store
            .prune_expired(exact_cutoff + Duration::nanoseconds(1), 500)
            .await
            .expect("expired"),
        1
    );
    assert!(store.interaction("new-request").await.is_none());
    let mut observer = sqlite_scenarios::observer(directory.path()).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM typed_interaction_history")
        .fetch_one(&mut observer)
        .await
        .expect("durable prune");
    assert_eq!(count, 0);
}
