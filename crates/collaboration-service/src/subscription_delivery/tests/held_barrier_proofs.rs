//! The test-only storage hold resumes on guard drop and remains cancellable at shutdown.
use super::super::subscription_service::OwnerObservation;
use super::owner_fixture::*;
use collaboration_protocol::{DeliveryOutcome, PushDeliveryState};
use std::time::Duration;

const OWNER_COMPLETION_TIMEOUT: Duration = Duration::from_secs(30);

#[tokio::test]
async fn dropped_hold_after_observation_error_resumes_the_same_owner() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    let failed_observation = async {
        let _hold = runtime
            .hold_reader_at_storage_boundary(&fixture.reader)
            .await;
        fixture
            .post(&fixture.root, "Arrives while observation is held")
            .await;
        assert!(runtime.requests.lock().await.try_recv().is_err());
        Err::<(), &str>("injected fixture observation error")
    }
    .await;
    assert_eq!(
        failed_observation,
        Err("injected fixture observation error")
    );
    let request = tokio::time::timeout(OWNER_COMPLETION_TIMEOUT, async {
        runtime.requests.lock().await.recv().await
    })
    .await
    .expect("guard drop resumes delivery")
    .expect("same owner sends a request");
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    let owner_starts = tokio::time::timeout(OWNER_COMPLETION_TIMEOUT, async {
        let mut observations = runtime.observations.lock().await;
        let mut starts = 0;
        loop {
            let event = observations.recv().await.unwrap();
            eprintln!("held-barrier drop: {event:?}");
            starts += usize::from(matches!(event, OwnerObservation::Started));
            if matches!(event, OwnerObservation::Settled) {
                return starts;
            }
        }
    })
    .await
    .expect("resumed owner settles the real delivery");
    assert_eq!(
        owner_starts, 1,
        "observation error must not replace the owner"
    );
    let stored = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.delivery_state, PushDeliveryState::Delivered);
    runtime.close().await;
}

#[tokio::test]
async fn shutdown_cancels_a_hold_while_its_release_guard_is_retained() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    let hold = runtime
        .hold_reader_at_storage_boundary(&fixture.reader)
        .await;
    fixture
        .post(&fixture.root, "Pending when held owner shuts down")
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let shutdown = tokio::time::timeout(OWNER_COMPLETION_TIMEOUT, runtime.close()).await;
    // The guard was still owned throughout shutdown, so only cancellation can unblock it.
    drop(hold);
    assert!(
        shutdown.is_ok(),
        "shutdown must bypass the retained test hold"
    );
}
