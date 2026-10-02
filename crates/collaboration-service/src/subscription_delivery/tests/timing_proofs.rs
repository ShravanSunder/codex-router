use super::super::SubscriptionClock;
use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use message_board::{SubscriptionMode, WhenIdle};

#[tokio::test(start_paused = true)]
async fn quiet_period_restarts_from_the_second_arrival() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            SubscriptionMode::Deliver,
            WhenIdle::Hold,
            120,
            600,
        )
        .await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "First").await;
    runtime.synchronize(&fixture.reader).await;
    runtime.observe(|event| matches!(event, OwnerObservation::Sleeping(Some(deadline)) if *deadline == fixture.clock.monotonic_now() + std::time::Duration::from_secs(120))).await;
    fixture.clock.advance(60).await;
    fixture.post(&fixture.root, "Second").await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(119).await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(1).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("2 messages"));
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn repeated_ninety_second_arrivals_are_passed_at_the_ten_minute_cap() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            SubscriptionMode::Deliver,
            WhenIdle::Hold,
            120,
            600,
        )
        .await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Opening").await;
    runtime.synchronize(&fixture.reader).await;
    for _ in 0..6 {
        fixture.clock.advance(90).await;
        fixture.post(&fixture.root, "Continuing").await;
        runtime.synchronize(&fixture.reader).await;
    }
    fixture.clock.advance(59).await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(1).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("7 messages"));
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn activity_in_one_root_never_delays_another_root() {
    let fixture = OwnerFixture::new().await;
    let other = fixture.add_root().await;
    for root in [&fixture.root, &other] {
        fixture
            .policy(root, SubscriptionMode::Deliver, WhenIdle::Hold, 120, 600)
            .await;
    }
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "First root").await;
    fixture.post(&other, "Other root").await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(60).await;
    fixture.post(&other, "Other extended").await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(60).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    let record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        record.activity.unwrap().ranges[0].root_message_id,
        fixture.root
    );
    runtime.await_settled().await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(60).await;
    runtime.requests.lock().await.recv().await.unwrap();
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn rejected_delivery_backoff_doubles_from_thirty_seconds_to_ten_minutes() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Pending").await;
    for delay in [30, 60, 120, 240, 480, 600, 600] {
        runtime.requests.lock().await.recv().await.unwrap();
        runtime
            .completions
            .send(collaboration_protocol::DeliveryOutcome::NotSubmitted {
                retryable: false,
                reason: "target rejects input".to_owned(),
            })
            .unwrap();
        runtime
            .observe(|event| matches!(event, OwnerObservation::RetryScheduled))
            .await;
        let record = fixture
            .store
            .lock()
            .await
            .get_thread_subscription_record(
                &fixture.reader,
                &message_board::SubscriptionScope::thread(fixture.root.clone()),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            record.roots()[0].next_retry_at().unwrap(),
            fixture.clock.now() + chrono::Duration::seconds(i64::try_from(delay).unwrap())
        );
        fixture.clock.advance(delay - 1).await;
        runtime.synchronize(&fixture.reader).await;
        assert!(runtime.requests.lock().await.try_recv().is_err());
        fixture.clock.advance(1).await;
    }
    runtime.requests.lock().await.recv().await.unwrap();
    runtime.await_settled().await;
    runtime.close().await;
}
