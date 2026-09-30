#![allow(clippy::expect_used)]

use automation_storage::{AutomationStore, LatestAgentSenderRecord, StorageError};
use chrono::{Duration, Utc};
use collaboration_protocol::{EndpointId, EndpointRef, SessionId, SessionRef, UuidIdentity};
use sqlx::{Connection, sqlite::SqliteConnectOptions};

fn session(endpoint_id: &str, session_id: &str) -> SessionRef {
    SessionRef {
        endpoint: EndpointRef {
            service_id: UuidIdentity::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())
                .expect("service UUID"),
            endpoint_id: EndpointId::try_from(endpoint_id.to_owned()).expect("endpoint id"),
        },
        session_id: SessionId::try_from(session_id.to_owned()).expect("session id"),
    }
}

fn database_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "automation-latest-agent-sender-{}.sqlite",
        uuid::Uuid::now_v7()
    ))
}

#[tokio::test]
async fn latest_sender_overwrites_persists_after_reopen_and_prunes_after_thirty_days() {
    let path = database_path();
    let recipient = session("codex-local", "recipient-thread");
    let first_sender = session("claude-local", "first-sender");
    let second_sender = session("cursor-local", "second-sender");
    let expired_recipient = session("codex-local", "expired-recipient");
    let now = Utc::now();
    let current_record = LatestAgentSenderRecord::new(
        recipient.clone(),
        first_sender.clone(),
        now - Duration::seconds(1),
    )
    .expect("valid current record");
    let expired_record = LatestAgentSenderRecord::new(
        expired_recipient.clone(),
        first_sender,
        now - Duration::days(31),
    )
    .expect("valid expired record");

    let mut store = AutomationStore::open(&path).await.expect("open store");
    store
        .store_latest_agent_sender(&current_record)
        .await
        .expect("persist sender record");
    store
        .store_latest_agent_sender(&expired_record)
        .await
        .expect("persist expired fixture");
    store.close().await.expect("close first store instance");

    let mut reopened = AutomationStore::open(&path).await.expect("reopen store");
    let after_reopen = reopened
        .latest_agent_sender_for(&recipient, now)
        .await
        .expect("read persisted sender")
        .expect("sender exists after reopen");
    assert_eq!(after_reopen.sender, current_record.sender);

    let replacement = LatestAgentSenderRecord::new(recipient.clone(), second_sender.clone(), now)
        .expect("valid replacement");
    reopened
        .store_latest_agent_sender(&replacement)
        .await
        .expect("overwrite latest sender");
    assert_eq!(
        reopened
            .latest_agent_sender_for(&recipient, now)
            .await
            .expect("read latest sender")
            .expect("sender exists")
            .sender,
        second_sender
    );
    assert!(
        reopened
            .latest_agent_sender_for(&expired_recipient, now)
            .await
            .expect("old sender is outside the reply window")
            .is_none()
    );
    assert_eq!(
        reopened
            .prune_latest_agent_senders(now, 1000)
            .await
            .expect("prune expired senders"),
        1
    );
    reopened.close().await.expect("close reopened store");
    std::fs::remove_file(&path).expect("remove isolated database");
}

#[tokio::test]
async fn invalid_persisted_sender_identity_fails_closed() {
    let path = database_path();
    let recipient = session("codex-local", "recipient-thread");
    let record = LatestAgentSenderRecord::new(
        recipient.clone(),
        session("claude-local", "sender-thread"),
        Utc::now(),
    )
    .expect("valid sender record");
    let mut store = AutomationStore::open(&path).await.expect("open store");
    store
        .store_latest_agent_sender(&record)
        .await
        .expect("persist sender record");
    store
        .close()
        .await
        .expect("close store before corruption fixture");

    let mut observer = sqlx::SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .foreign_keys(true),
    )
    .await
    .expect("open isolated observer");
    sqlx::query(
        "UPDATE latest_agent_senders SET sender_endpoint_id='Claude' WHERE recipient_session_id=?",
    )
    .bind(String::from(recipient.session_id.clone()))
    .execute(&mut observer)
    .await
    .expect("write corrupt endpoint fixture");
    observer.close().await.expect("close observer");

    let mut reopened = AutomationStore::open(&path).await.expect("reopen store");
    let result = reopened
        .latest_agent_sender_for(&recipient, Utc::now())
        .await;
    reopened.close().await.expect("close reopened store");
    std::fs::remove_file(&path).expect("remove isolated database");

    assert!(matches!(result, Err(StorageError::InvalidRecord)));
}
