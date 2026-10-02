//! Public Control views must expose the owner's durable hold and retry facts.
use super::super::SubscriptionClock;
use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use collaboration_protocol::{DeliveryOutcome, ThreadSubscriptionsRequest};
use message_board::SubscriptionDeliveryOutcome;

#[tokio::test]
async fn control_subscriptions_view_reports_pending_held_outcome_and_retry_deadline() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Wakeable;
    let runtime = fixture.runtime().await;
    let identity = crate::ServiceIdentity::new(
        SERVICE_ID,
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap()
    .with_board_store(fixture.store.clone())
    .with_subscription_delivery_service(runtime.service.clone(), fixture.presence.clone());
    let (socket, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = tokio::spawn(crate::serve_control_connection(server, identity));
    let mut client = collaboration_client::ControlClient::initialize(socket, "view-proof", "1")
        .await
        .unwrap();
    fixture.post(&fixture.root, "Pending view activity").await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    let held = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: fixture.reader.clone(),
        })
        .await
        .unwrap()
        .subscriptions;
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].pending_count, 1);
    assert_eq!(held[0].held_since, Some(fixture.clock.now()));
    assert_eq!(held[0].next_retry_at, None);
    assert_eq!(
        held[0].last_outcome,
        Some(SubscriptionDeliveryOutcome::NotSubmitted {
            reason: "target is not running".to_owned(),
            retryable: true,
        })
    );
    assert_eq!(
        held[0].presence,
        collaboration_protocol::ThreadSubscriptionPresence::Wakeable {}
    );

    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Running;
    fixture.clock.advance_without_tokio_time(30);
    runtime.requests.lock().await.recv().await.unwrap();
    runtime
        .completions
        .send(DeliveryOutcome::NotSubmitted {
            retryable: false,
            reason: "provider rejected attempt".to_owned(),
        })
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::RetryScheduled))
        .await;
    let retry = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: fixture.reader.clone(),
        })
        .await
        .unwrap()
        .subscriptions;
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].pending_count, 1);
    assert_eq!(retry[0].held_since, None);
    assert_eq!(
        retry[0].next_retry_at,
        Some(fixture.clock.now() + chrono::Duration::seconds(30))
    );
    assert_eq!(
        retry[0].last_outcome,
        Some(SubscriptionDeliveryOutcome::NotSubmitted {
            reason: "provider rejected attempt".to_owned(),
            retryable: false,
        })
    );
    assert_eq!(
        retry[0].presence,
        collaboration_protocol::ThreadSubscriptionPresence::Running {}
    );
    drop(client);
    serving.await.unwrap().unwrap();
    runtime.close().await;
}
