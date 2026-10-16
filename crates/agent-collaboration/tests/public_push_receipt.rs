#[path = "delivery_matrix/public_push_receipt.rs"]
mod public_push_receipt;

use collaboration_protocol::{PushDeliveryState, PushRecord};
use public_push_receipt::{ReceiptResult, wait_for_public_receipt};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::time::Instant;

fn attempted_push() -> ReceiptResult<PushRecord> {
    Ok(serde_json::from_value(json!({
        "pushId":"11111111-1111-7111-8111-111111111111", "kind":"wake",
        "origin":{"router":"wake"},
        "target":{"endpoint":{"serviceId":"22222222-2222-7222-8222-222222222222","endpointId":"codex-local"},"sessionId":"recipient"},
        "replyToPushId":null,"headerFacts":{"kind":"wake"},"body":"wake marker",
        "activity":null,"deliveryState":"attempted","lastOutcome":null,
        "createdAt":"2026-10-16T00:00:00Z","settledAt":null,"readAt":null
    }))?)
}

#[tokio::test(start_paused = true)]
async fn input_visibility_before_receipt_publication_does_not_end_observation() {
    let published = Arc::new(AtomicBool::new(false));
    let observed_publication = Arc::clone(&published);
    let (first_read, observed_first_read) = tokio::sync::oneshot::channel();
    let mut first_read = Some(first_read);
    let observation = tokio::spawn(async move {
        wait_for_public_receipt(
            async move || {
                let mut record = attempted_push()?;
                if observed_publication.load(Ordering::SeqCst) {
                    record.delivery_state = PushDeliveryState::Delivered;
                }
                if let Some(signal) = first_read.take() {
                    let _ = signal.send(());
                }
                Ok(record)
            },
            Instant::now() + Duration::from_secs(5),
        )
        .await
    });
    observed_first_read
        .await
        .expect("first public snapshot observed");
    published.store(true, Ordering::SeqCst);
    let record = observation
        .await
        .expect("observation task")
        .expect("published receipt");
    assert_eq!(record.delivery_state, PushDeliveryState::Delivered);
}

#[tokio::test(start_paused = true)]
async fn unpublished_receipt_expires_at_the_original_deadline() {
    let started = Instant::now();
    let result =
        wait_for_public_receipt(async || attempted_push(), started + Duration::from_secs(5)).await;
    assert!(result.is_err());
    assert_eq!(Instant::now() - started, Duration::from_secs(5));
}

#[tokio::test]
async fn public_read_errors_are_not_retried() {
    let reads = AtomicUsize::new(0);
    let result = wait_for_public_receipt(
        async || {
            reads.fetch_add(1, Ordering::SeqCst);
            Err("public read failed".into())
        },
        Instant::now() + Duration::from_secs(5),
    )
    .await;
    assert!(result.is_err());
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn terminal_unknown_returns_for_correlation_failure_without_resubmission() {
    let result = wait_for_public_receipt(
        async || {
            let mut record = attempted_push()?;
            record.delivery_state = PushDeliveryState::OutcomeUnknown;
            Ok(record)
        },
        Instant::now() + Duration::from_secs(5),
    )
    .await
    .expect("terminal observation");
    assert_eq!(result.delivery_state, PushDeliveryState::OutcomeUnknown);
}
