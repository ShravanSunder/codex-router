use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use crate::{LoadPolicy, TargetPresence};
use message_board::{EndReason, SubscriptionMode, SubscriptionScope, WhenIdle};

#[tokio::test(start_paused = true)]
async fn hold_keeps_all_arrivals_pending_and_delivers_one_record_on_running() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "First held").await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    runtime.synchronize(&fixture.reader).await;
    fixture.post(&fixture.root, "Second held").await;
    fixture.post(&fixture.root, "Third held").await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    *fixture.presence.0.lock().unwrap() = TargetPresence::Running;
    fixture.clock.advance(29).await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(1).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert_eq!(request.payload.load_policy, LoadPolicy::LoadedOnly);
    assert!(request.payload.line.as_str().contains("3 messages"));
    assert!(request.payload.line.as_str().contains("held since"));
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert!(record.activity.unwrap().held);
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn wake_uses_may_load_only_for_a_wakeable_target() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            SubscriptionMode::Deliver,
            WhenIdle::Wake,
            0,
            0,
        )
        .await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Wake me").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert_eq!(request.payload.load_policy, LoadPolicy::MayLoad);
    runtime.await_settled().await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Running;
    fixture.post(&fixture.root, "Running").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert_eq!(request.payload.load_policy, LoadPolicy::LoadedOnly);
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn unreachable_wake_is_held_with_visible_reason() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            SubscriptionMode::Deliver,
            WhenIdle::Wake,
            0,
            0,
        )
        .await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Unreachable {
        reason: "closed terminal".to_owned(),
    };
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Pending").await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    let record = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(
        serde_json::to_string(&record.last_outcome())
            .unwrap()
            .contains("wake unavailable; holding")
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn drop_then_push_leaves_both_messages_unread() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            SubscriptionMode::Deliver,
            WhenIdle::Drop,
            0,
            0,
        )
        .await;
    *fixture.presence.0.lock().unwrap() = TargetPresence::Wakeable;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Skipped").await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    *fixture.presence.0.lock().unwrap() = TargetPresence::Running;
    fixture.post(&fixture.root, "Pushed").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        record.header_facts,
        collaboration_protocol::PushHeaderFacts::SubscriptionActivity {
            message_count: 1,
            ..
        }
    ));
    runtime.await_settled().await;
    assert_eq!(fixture.unread_count().await, 2);
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn expiry_without_activity_sends_one_stored_renew_command() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(24 * 60 * 60).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(
        request
            .payload
            .line
            .as_str()
            .starts_with("🧵 Router: subscription expired")
    );
    assert!(!request.payload.line.as_str().contains("subscribe"));
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.kind,
        collaboration_protocol::PushKind::SubscriptionExpiry
    );
    assert!(
        record
            .body
            .unwrap()
            .contains("board thread subscribe --root-message-id")
    );
    runtime
        .completions
        .send(collaboration_protocol::DeliveryOutcome::Started)
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Stopped))
        .await;
    let record = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.state().end_reason(), Some(EndReason::Expired));
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn unknown_outcome_advances_delivered_and_preserves_evidence() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Uncertain").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime
        .completions
        .send(collaboration_protocol::DeliveryOutcome::Unknown)
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let stored = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.delivery_state,
        collaboration_protocol::PushDeliveryState::OutcomeUnknown
    );
    let record = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        record.last_outcome(),
        Some(message_board::SubscriptionDeliveryOutcome::Unknown { .. })
    ));
    assert_eq!(fixture.unread_count().await, 1);
    runtime.close().await;
}
