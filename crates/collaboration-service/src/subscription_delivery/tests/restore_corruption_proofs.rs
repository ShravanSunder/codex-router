//! One corrupt recovery row must not suppress healthy readers at Host startup.
use super::super::{
    SubscriptionClock, subscription_push::target_session, subscription_service::OwnerObservation,
};
use super::owner_fixture::*;
use collaboration_protocol::{
    MessageDelivery, PushDeliveryState, PushHeaderFacts, PushId, PushKind, PushOrigin, PushRecord,
    PushRecordDraft, SessionId, SessionRef,
};
use message_board::*;
use sqlx::Connection;
use std::time::Duration;

fn direct_message_draft(
    target: &SessionRef,
    push_body: &str,
    created_at: chrono::DateTime<chrono::Utc>,
) -> PushRecordDraft {
    PushRecordDraft {
        push_id: PushId::try_from(uuid::Uuid::now_v7().to_string()).unwrap(),
        kind: PushKind::DirectMessage,
        origin: PushOrigin::OwnerUnverified,
        origin_router_ref: None,
        target: target.clone(),
        mode: Some(MessageDelivery::Auto),
        guard: None,
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::DirectMessage {
            sender_display_name: None,
        },
        body: Some(push_body.to_owned()),
        activity: None,
        created_at,
    }
}

async fn stored_push(fixture: &OwnerFixture, push_id: &PushId) -> PushRecord {
    fixture
        .push_store
        .lock()
        .await
        .get_push_record(push_id)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn corrupt_subscription_mode_is_skipped_while_valid_reader_restores_and_delivers() {
    let fixture = OwnerFixture::new().await;
    let corrupt_reader = human("corrupt-reader");
    fixture
        .store
        .lock()
        .await
        .join_thread(
            ThreadJoinRequest {
                root_message_id: fixture.root.clone(),
                actor: corrupt_reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                mode: None,
                when_idle: None,
                replace: None,
                note: None,
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    fixture
        .store
        .lock()
        .await
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: corrupt_reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    ..SubscriptionPolicyPatch::default()
                },
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap();
    fixture
        .post(&fixture.root, "Healthy reader pending at restart")
        .await;
    sqlx::query("UPDATE thread_subscriptions SET mode='corrupt-mode' WHERE reader_key IN (SELECT identity_key FROM board_identities WHERE human_id='corrupt-reader')")
        .execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("1 message"));
    runtime.await_settled().await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert!(
        fixture
            .store
            .lock()
            .await
            .get_thread_subscription_record(
                &fixture.reader,
                &SubscriptionScope::thread(fixture.root.clone())
            )
            .await
            .unwrap()
            .unwrap()
            .roots()
            .is_empty()
    );
    let corrupt = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &corrupt_reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(corrupt.kind, BoardFailureKind::InvalidRecord);
    assert!(corrupt.message.contains("'mode'"));
    let stored_mode: String = sqlx::query_scalar("SELECT mode FROM thread_subscriptions WHERE reader_key IN (SELECT identity_key FROM board_identities WHERE human_id='corrupt-reader')")
        .fetch_one(&mut observer).await.unwrap();
    assert_eq!(
        stored_mode, "corrupt-mode",
        "corrupt row must not be silently repaired"
    );
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_direct_message_recovery_query_does_not_stop_valid_subscription_reader() {
    let fixture = OwnerFixture::new().await;
    fixture
        .post(&fixture.root, "Healthy reader despite DM query failure")
        .await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    // The connection remains usable; only the DM recovery repository query is unavailable.
    sqlx::query("ALTER TABLE router_pushes RENAME TO temporarily_unavailable_pushes")
        .execute(&mut observer)
        .await
        .unwrap();
    let runtime = fixture.runtime().await;
    sqlx::query("ALTER TABLE temporarily_unavailable_pushes RENAME TO router_pushes")
        .execute(&mut observer)
        .await
        .unwrap();
    // Wake the reader after the query becomes available; no Host/service restart.
    runtime
        .service
        .reconcile_reader(fixture.reader.clone())
        .await
        .unwrap();
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("1 message"));
    runtime.await_settled().await;
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test]
async fn corrupt_recovery_target_row_does_not_strand_held_dm_after_restart() {
    let fixture = OwnerFixture::new().await;
    let target = target_session(&fixture.reader).unwrap();
    let held_draft = direct_message_draft(&target, "held DM survives restart", fixture.clock.now());
    let held_push_id = held_draft.push_id.clone();
    fixture
        .push_store
        .lock()
        .await
        .insert_push_record(held_draft)
        .await
        .unwrap();
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Wakeable;
    let first_runtime = fixture.runtime_without_board().await;
    first_runtime
        .observe(|event| matches!(event, OwnerObservation::DirectMessageHeld))
        .await;
    assert_eq!(
        stored_push(&fixture, &held_push_id).await.delivery_state,
        PushDeliveryState::Held
    );
    first_runtime.close().await;

    let mut corrupt_target = target.clone();
    corrupt_target.session_id = SessionId::try_from("corrupt-recovery-target".to_owned()).unwrap();
    let corrupt_draft =
        direct_message_draft(&corrupt_target, "invalid target row", fixture.clock.now());
    let corrupt_push_id = corrupt_draft.push_id.clone();
    fixture
        .push_store
        .lock()
        .await
        .insert_push_record(corrupt_draft)
        .await
        .unwrap();
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE router_pushes SET target_session_id='' WHERE push_id=?")
        .bind(corrupt_push_id.as_str())
        .execute(&mut observer)
        .await
        .unwrap();

    let recovery_targets = fixture
        .push_store
        .lock()
        .await
        .direct_message_recovery_targets()
        .await
        .unwrap();
    assert_eq!(recovery_targets, vec![target.clone()]);

    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Running;
    let recovered_runtime = fixture.runtime_without_board().await;
    let request = tokio::time::timeout(Duration::from_secs(5), async {
        recovered_runtime
            .requests
            .lock()
            .await
            .recv()
            .await
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(request.target, target);
    assert_eq!(request.payload.push_id, held_push_id);
    assert!(
        request
            .payload
            .line
            .as_str()
            .contains("held DM survives restart")
    );
    recovered_runtime
        .completions
        .send(collaboration_protocol::DeliveryOutcome::Started)
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        recovered_runtime.observe(|event| matches!(event, OwnerObservation::DirectMessageSettled)),
    )
    .await
    .expect("restarted held DM should settle");
    assert_eq!(
        stored_push(&fixture, &held_push_id).await.delivery_state,
        PushDeliveryState::Delivered
    );
    let invalid_target_session: String =
        sqlx::query_scalar("SELECT target_session_id FROM router_pushes WHERE push_id=?")
            .bind(corrupt_push_id.as_str())
            .fetch_one(&mut observer)
            .await
            .unwrap();
    assert_eq!(invalid_target_session, "", "corrupt row stays unchanged");

    observer.close().await.unwrap();
    recovered_runtime.close().await;
}
