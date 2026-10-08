use super::super::{InteractionHistoryRecord, RefusedApprovalOption, RefusedTypedApproval};
use super::InteractionHistoryStore;
use chrono::{Duration, Utc};
use message_board::{Identity, SessionRef};
use std::collections::BTreeMap;

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
async fn sqlite_import_reopens_records_without_rewriting_original_json() {
    let directory = tempfile::tempdir().expect("isolated import");
    let source = directory.path().join("interaction-history.json");
    let original = serde_json::to_vec_pretty(&BTreeMap::from([(
        "imported-refusal",
        refused_approval("imported-refusal"),
    )]))
    .expect("source fixture");
    tokio::fs::write(&source, &original).await.expect("source");
    let store = InteractionHistoryStore::load(source.clone())
        .await
        .expect("import");
    let database = directory.path().join("interaction.sqlite");
    assert!(
        database.is_file(),
        "typed history must use the SQLite database"
    );
    assert_eq!(
        tokio::fs::read(&source).await.expect("recovery copy"),
        original
    );
    let first_stamp = store.data.lock().await.created_at["imported-refusal"];
    store
        .record_refused_approval(
            session_ref("requester"),
            Identity::Session {
                session: session_ref("approver"),
            },
            RefusedTypedApproval {
                request_id: "new-refusal".into(),
                title: "Refused".into(),
                description: None,
                subject: None,
                options: vec![],
                reason: "fixture".into(),
            },
        )
        .await
        .expect("SQLite mutation");
    drop(store);
    let reopened = InteractionHistoryStore::load(source.clone())
        .await
        .expect("reopen");
    assert!(reopened.interaction("new-refusal").await.is_some());
    assert_eq!(
        reopened.data.lock().await.created_at["imported-refusal"],
        first_stamp
    );
    assert_eq!(
        tokio::fs::read(source)
            .await
            .expect("unchanged recovery copy"),
        original
    );
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(database)
        .read_only(true);
    let mut observer = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(&options)
        .await
        .expect("independent observer");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM interaction_history_records")
        .fetch_one(&mut observer)
        .await
        .expect("durable rows");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn duplicate_import_keys_are_rejected_without_rewriting_source() {
    let directory = tempfile::tempdir().expect("isolated duplicate fixture");
    let source = directory.path().join("interaction-history.json");
    let row = serde_json::to_string(&refused_approval("same-id")).expect("row");
    let original = format!("{{\"same-id\":{row},\"same-id\":{row}}}").into_bytes();
    tokio::fs::write(&source, &original)
        .await
        .expect("duplicate source");
    assert!(
        InteractionHistoryStore::load(source.clone()).await.is_err(),
        "duplicate keys must fail rather than silently select one record"
    );
    assert_eq!(
        tokio::fs::read(source)
            .await
            .expect("preserved invalid source"),
        original
    );
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
        InteractionHistoryStore::load(directory.path().join("interaction-history.json"))
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
async fn load_stamps_undated_entries_and_prunes_strictly_after_thirty_days() {
    let directory = tempfile::tempdir().expect("isolated interaction history");
    let path = directory.path().join("interaction-history.json");
    let legacy = BTreeMap::from([(
        "legacy-request".to_owned(),
        refused_approval("legacy-request"),
    )]);
    tokio::fs::write(
        &path,
        serde_json::to_vec_pretty(&legacy).expect("serialize legacy interaction history"),
    )
    .await
    .expect("write undated legacy history");
    let load_started_at = Utc::now();

    let store = InteractionHistoryStore::load(path.clone())
        .await
        .expect("load and upgrade history");
    let created_at = store
        .data
        .lock()
        .await
        .created_at
        .get("legacy-request")
        .copied()
        .expect("stamp the old entry at upgrade");
    assert!(created_at >= load_started_at && created_at <= Utc::now());
    let unchanged: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&path).await.expect("read recovery history"))
            .expect("parse original history");
    assert!(unchanged["legacy-request"].get("createdAt").is_none());

    let exact_cutoff = created_at + Duration::days(30);
    assert_eq!(
        store
            .prune_expired(exact_cutoff, 500)
            .await
            .expect("preserve exact cutoff"),
        0
    );
    assert_eq!(
        store
            .prune_expired(exact_cutoff + Duration::nanoseconds(1), 500)
            .await
            .expect("prune one expired entry"),
        1
    );
    assert!(store.interaction("legacy-request").await.is_none());
    store
        .record_refused_approval(
            session_ref("requester"),
            Identity::Session {
                session: session_ref("approver"),
            },
            RefusedTypedApproval {
                request_id: "new-request".to_owned(),
                title: "New refusal".to_owned(),
                description: None,
                subject: None,
                options: vec![],
                reason: "fixture refusal".to_owned(),
            },
        )
        .await
        .expect("record new interaction with a timestamp");
    let mut observer = <sqlx::SqliteConnection as sqlx::Connection>::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path.with_file_name("interaction.sqlite"))
            .read_only(true),
    )
    .await
    .expect("SQLite observer");
    let persisted: String = sqlx::query_scalar(
        "SELECT created_at FROM interaction_history_records WHERE request_id='new-request'",
    )
    .fetch_one(&mut observer)
    .await
    .expect("creation timestamp");
    assert!(super::parse_history_timestamp(&persisted).is_ok());
}
