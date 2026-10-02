use super::super::SubscriptionClock;
use super::super::subscription_push::target_session;
use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use collaboration_protocol::{DeliveryOutcome, PushId, UuidIdentity};
use message_board::{SubscriptionScope, ThreadSubscriptionUnsubscribeRequest};
use serde_json::{Value, json};

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
