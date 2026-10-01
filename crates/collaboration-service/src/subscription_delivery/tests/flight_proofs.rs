use super::owner_fixture::*;
use crate::control_service_context::subscription_delivery::subscription_service::OwnerObservation;
use crate::control_service_context::subscription_delivery::{
    SubscriptionClock, SubscriptionWaitFilter, SubscriptionWaitResult,
};
use message_board::{EndReason, SubscriptionMode, SubscriptionScope, WhenIdle};
use sqlx::Connection;

#[tokio::test(start_paused = true)]
async fn one_owner_serializes_push_and_wait_and_does_not_hold_the_board_lock() {
    let fixture = OwnerFixture::new().await;
    let poll_root = fixture.add_root().await;
    fixture
        .policy(&poll_root, SubscriptionMode::Poll, WhenIdle::Hold, 0, 0)
        .await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Push").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    fixture.post(&poll_root, "Poll").await; // This real write proves the route await released the board lock.
    let service = runtime.service.clone();
    let reader = fixture.reader.clone();
    let mut wait = tokio::spawn(async move {
        service
            .wait(
                reader,
                SubscriptionWaitFilter::Roots(vec![poll_root]),
                60,
                usize::MAX,
            )
            .await
            .unwrap()
    });
    assert!(!wait.is_finished());
    runtime.await_settled().await;
    let notice = (&mut wait).await.unwrap().unwrap();
    let SubscriptionWaitResult::Notice { push_id, batch, .. } = notice else {
        panic!("session waits have stored notices");
    };
    assert_ne!(push_id, request.payload.push_id);
    assert_eq!(batch.roots.len(), 1);
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn twenty_root_limit_drains_remaining_due_roots_immediately() {
    let fixture = OwnerFixture::new().await;
    let mut roots = vec![fixture.root.clone()];
    for _ in 0..20 {
        roots.push(fixture.add_root().await);
    }
    for root in &roots {
        fixture.post(root, "Pending").await;
    }
    let runtime = fixture.runtime().await;
    let first = runtime.requests.lock().await.recv().await.unwrap();
    let first_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&first.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    let first_ranges = first_record.activity.unwrap().ranges;
    assert_eq!(first_ranges.len(), 20);
    runtime.await_settled().await;
    let second = runtime.requests.lock().await.recv().await.unwrap();
    let second_record = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&second.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    let second_ranges = second_record.activity.unwrap().ranges;
    assert_eq!(second_ranges.len(), 1);
    assert!(
        !first_ranges
            .iter()
            .any(|range| range.root_message_id == second_ranges[0].root_message_id)
    );
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn arrival_during_flight_gets_its_own_quiet_window_and_elapsed_cap() {
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
    fixture.post(&fixture.root, "Original").await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(120).await;
    let first = runtime.requests.lock().await.recv().await.unwrap();
    fixture.post(&fixture.root, "First residual").await;
    for _ in 0..6 {
        fixture.clock.advance(90).await;
        fixture.post(&fixture.root, "Residual extends quiet").await;
    }
    fixture.clock.advance(60).await; // Cap from the first residual has elapsed while the first push was in flight.
    assert!(runtime.requests.lock().await.try_recv().is_err());
    runtime.await_settled().await;
    let second = runtime.requests.lock().await.recv().await.unwrap();
    assert_ne!(first.payload.push_id, second.payload.push_id);
    assert!(second.payload.line.as_str().contains("7 messages"));
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn cancel_during_flight_settles_the_passed_range_and_prevents_later_pushes() {
    let fixture = OwnerFixture::new().await;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "In flight").await;
    runtime.requests.lock().await.recv().await.unwrap();
    fixture
        .store
        .lock()
        .await
        .unsubscribe_thread_subscription(
            message_board::ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    runtime.await_settled().await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Stopped))
        .await;
    fixture.post(&fixture.root, "After cancel").await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert_eq!(fixture.unread_count().await, 2);
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn board_settlement_failure_retries_without_redelivery() {
    let fixture = OwnerFixture::new().await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_owner_settle BEFORE INSERT ON thread_delivery_positions BEGIN SELECT RAISE(FAIL, 'injected settlement failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture
        .post(&fixture.root, "Accepted before recording")
        .await;
    runtime.requests.lock().await.recv().await.unwrap();
    runtime
        .completions
        .send(collaboration_protocol::DeliveryOutcome::Started)
        .unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::SettlementRetry))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    sqlx::query("DROP TRIGGER fail_owner_settle")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance(1).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn resolved_held_subscription_drains_then_ends_resolved() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Wakeable;
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Held final activity").await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Held))
        .await;
    fixture
        .store
        .lock()
        .await
        .resolve_thread(
            message_board::ThreadResolveRequest {
                root_message_id: fixture.root.clone(),
                actor: human("board-owner"),
                acting_for: None,
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    runtime.synchronize(&fixture.reader).await;
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Running;
    fixture.clock.advance(30).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("thread resolved"));
    runtime.await_settled().await;
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
    assert_eq!(record.state().end_reason(), Some(EndReason::Resolved));
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn restart_mid_quiet_preserves_the_original_arrival_deadline() {
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
    fixture.post(&fixture.root, "Before restart").await;
    runtime.synchronize(&fixture.reader).await;
    fixture.clock.advance(60).await;
    runtime.synchronize(&fixture.reader).await;
    runtime.close().await;
    reopen_board(&fixture).await;
    let runtime = fixture.runtime().await;
    fixture.clock.advance(59).await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(1).await;
    runtime.requests.lock().await.recv().await.unwrap();
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn restart_after_cap_delivers_immediately_with_downtime_arrivals_included() {
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
    fixture.post(&fixture.root, "Before downtime").await;
    runtime.synchronize(&fixture.reader).await;
    runtime.close().await;
    for _ in 0..7 {
        fixture.clock.advance(90).await;
        fixture.post(&fixture.root, "During downtime").await;
    }
    reopen_board(&fixture).await;
    let runtime = fixture.runtime().await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("8 messages"));
    runtime.await_settled().await;
    runtime.close().await;
}

async fn reopen_board(fixture: &OwnerFixture) {
    let reopened =
        message_board_storage::BoardStore::open(&fixture.directory.path().join("board.sqlite"))
            .await
            .unwrap();
    let old = std::mem::replace(&mut *fixture.store.lock().await, reopened);
    old.close().await.unwrap();
}
