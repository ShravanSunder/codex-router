use super::super::SubscriptionClock;
use super::super::subscription_push::target_session;
use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use collaboration_protocol::{DeliveryOutcome, PushId, UuidIdentity};
use message_board::{SubscriptionScope, ThreadSubscriptionUnsubscribeRequest};
use serde_json::{Value, json};
use sqlx::Connection;
use std::time::Duration;

// SQLite works on background threads; its acknowledgement bound needs real time.
#[tokio::test]
async fn cleanup_write_retry_does_not_block_reader_reconcile() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture
        .post(&fixture.root, "Held before ending scope")
        .await;
    let held_id = hold_next_push(&runtime).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(0)))
        .await;
    runtime.synchronize(&fixture.reader).await;
    while runtime.observations.lock().await.try_recv().is_ok() {}
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_held_cleanup BEFORE UPDATE OF delivery_state ON router_pushes WHEN OLD.delivery_state='held' AND NEW.delivery_state='rejected' BEGIN SELECT RAISE(FAIL, 'injected cleanup write failure'); END").execute(&mut observer).await.unwrap();
    fixture
        .store
        .lock()
        .await
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    runtime.reconcile(&fixture.reader).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Sleeping(Some(_))))
        .await;
    let acknowledged = tokio::time::timeout(
        Duration::from_secs(1),
        runtime.service.reconcile_reader(fixture.reader.clone()),
    )
    .await;
    assert!(
        matches!(acknowledged, Ok(Ok(()))),
        "cleanup write retry blocked a queued reconcile command"
    );
    runtime.synchronize(&fixture.reader).await;
    // A command acknowledges before the failed cleanup has been repaired.
    // The marker remains pending, and the next regular pass retries it.
    assert_eq!(
        fixture
            .push_store
            .lock()
            .await
            .get_push_record(&held_id)
            .await
            .unwrap()
            .unwrap()
            .delivery_state,
        collaboration_protocol::PushDeliveryState::Held
    );
    sqlx::query("DROP TRIGGER fail_held_cleanup")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance_without_tokio_time(30);
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(1)))
        .await;
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn delayed_batch_cleanup_never_supersedes_a_newer_held_push() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Batch before N").await;
    let old_id = hold_next_push(&runtime).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(0)))
        .await;
    runtime.synchronize(&fixture.reader).await;
    while runtime.observations.lock().await.try_recv().is_ok() {}
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_held_cleanup BEFORE UPDATE OF delivery_state ON router_pushes WHEN OLD.delivery_state='held' AND NEW.delivery_state='rejected' BEGIN SELECT RAISE(FAIL, 'injected cleanup write failure'); END").execute(&mut observer).await.unwrap();
    fixture.clock.advance(30).await;
    let batch_n = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Sleeping(Some(_))))
        .await;

    // Persist the N+1 held state while N's pending cleanup waits for another
    // owner pass. Eligibility must be recomputed without touching newer data.
    fixture.clock.advance(1).await;
    let mut store = fixture.push_store.lock().await;
    let old_record = store.get_push_record(&old_id).await.unwrap().unwrap();
    let newer_id = PushId::try_from(uuid::Uuid::now_v7().to_string()).unwrap();
    store
        .insert_push_record(collaboration_protocol::PushRecordDraft {
            push_id: newer_id.clone(),
            kind: old_record.kind,
            origin: old_record.origin,
            origin_router_ref: Some(
                collaboration_protocol::RouterOriginRef::SubscriptionActivity {
                    target: old_record.target.clone(),
                    batch_id: message_board::BatchId::generate(),
                }
                .canonical_string()
                .unwrap(),
            ),
            target: old_record.target,
            reply_to_push_id: None,
            header_facts: old_record.header_facts,
            body: None,
            activity: old_record.activity,
            mode: None,
            guard: None,
            created_at: fixture.clock.now(),
        })
        .await
        .unwrap();
    store.mark_push_attempted(&newer_id).await.unwrap();
    store
        .hold_push_record(
            &newer_id,
            collaboration_protocol::DeliveryReceipt {
                outcome: DeliveryOutcome::NotSubmitted {
                    retryable: true,
                    reason: "N+1 held".to_owned(),
                },
                reachability: None,
                client: None,
            },
        )
        .await
        .unwrap();
    drop(store);
    sqlx::query("DROP TRIGGER fail_held_cleanup")
        .execute(&mut observer)
        .await
        .unwrap();
    runtime.reconcile(&fixture.reader).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(_)))
        .await;
    let mut store = fixture.push_store.lock().await;
    assert_eq!(
        store
            .get_push_record(&newer_id)
            .await
            .unwrap()
            .unwrap()
            .delivery_state,
        collaboration_protocol::PushDeliveryState::Held,
        "N's delayed cleanup mislabeled N+1 as superseded"
    );
    assert_eq!(
        store
            .get_push_record(&old_id)
            .await
            .unwrap()
            .unwrap()
            .delivery_state,
        collaboration_protocol::PushDeliveryState::Rejected
    );
    assert_eq!(
        store
            .get_push_record(&batch_n.payload.push_id)
            .await
            .unwrap()
            .unwrap()
            .delivery_state,
        collaboration_protocol::PushDeliveryState::Delivered
    );
    drop(store);
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn ending_one_scope_preserves_a_held_batch_with_another_active_root() {
    let fixture = OwnerFixture::new().await;
    let active_root = fixture.add_root().await;
    fixture.post(&fixture.root, "Ending root").await;
    fixture.post(&active_root, "Active root").await;
    let runtime = fixture.runtime().await;
    let push_id = hold_next_push(&runtime).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(0)))
        .await;
    fixture
        .store
        .lock()
        .await
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    runtime.reconcile(&fixture.reader).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(_)))
        .await;
    assert_eq!(
        fixture
            .push_store
            .lock()
            .await
            .get_push_record(&push_id)
            .await
            .unwrap()
            .unwrap()
            .delivery_state,
        collaboration_protocol::PushDeliveryState::Held,
        "one ended scope must not settle activity for a live root"
    );
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn next_reader_batch_rejects_prior_held_push_with_current_link() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "First held batch").await;
    let first_push_id = hold_next_push(&runtime).await;

    fixture.clock.advance(30).await;
    let replacement = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(1)))
        .await;

    let identity = service_identity(&fixture);
    let first = show_record(&fixture, &identity, &first_push_id).await;
    assert_eq!(
        first.pointer("/result/record/deliveryState"),
        Some(&json!("rejected"))
    );
    assert_eq!(
        first.pointer("/result/record/lastOutcome/outcome/kind"),
        Some(&json!("notSubmitted"))
    );
    let replacement_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&replacement.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        first.pointer("/result/record/lastOutcome/outcome/reason"),
        Some(&Value::String(format!(
            "superseded by {}",
            crate::push_record_resolver::link_for(&replacement_record, &identity)
        )))
    );

    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn ended_subscription_rejects_its_held_push_with_end_reason() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Held before unsubscribe").await;
    let held_push_id = hold_next_push(&runtime).await;

    fixture
        .store
        .lock()
        .await
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    runtime.reconcile(&fixture.reader).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::HeldSubscriptionPushesSettled(1)))
        .await;

    let identity = service_identity(&fixture);
    let held = show_record(&fixture, &identity, &held_push_id).await;
    assert_eq!(
        held.pointer("/result/record/deliveryState"),
        Some(&json!("rejected"))
    );
    assert_eq!(
        held.pointer("/result/record/lastOutcome/outcome/reason"),
        Some(&json!("subscription ended: cancelled"))
    );

    runtime.close().await;
}

async fn hold_next_push(runtime: &OwnerRuntime) -> PushId {
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime
        .completions
        .send(DeliveryOutcome::NotSubmitted {
            retryable: true,
            reason: "target closed after presence check".to_owned(),
        })
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    request.payload.push_id
}

fn service_identity(fixture: &OwnerFixture) -> crate::ServiceIdentity {
    let service_id = UuidIdentity::try_from(SERVICE_ID.to_owned()).unwrap();
    crate::ServiceIdentity::new(
        SERVICE_ID,
        SERVICE_ID,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap()
    .with_machine_identity(
        crate::MachineIdentity::new(service_id, Some("held-push-test")).expect("machine identity"),
    )
    .unwrap()
    .with_automation_store(std::sync::Arc::clone(&fixture.push_store))
    .with_board_store(std::sync::Arc::clone(&fixture.store))
}

async fn show_record(
    fixture: &OwnerFixture,
    identity: &crate::ServiceIdentity,
    push_id: &PushId,
) -> Value {
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(push_id)
        .await
        .unwrap()
        .unwrap();
    crate::push_record_resolver::show(
        json!("show-held-push"),
        json!({
            "caller": target_session(&fixture.reader).unwrap(),
            "reference": crate::push_record_resolver::link_for(&record, identity),
        }),
        identity,
    )
    .await
}
