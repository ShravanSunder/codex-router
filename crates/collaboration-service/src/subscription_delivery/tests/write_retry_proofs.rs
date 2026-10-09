//! Failed pending-state writes must resolve their selected batch before new work.
use super::super::SubscriptionClock;
use super::owner_fixture::*;
use crate::service_identity::subscription_delivery::subscription_service::OwnerObservation;
use collaboration_protocol::DeliveryOutcome;
use message_board::SubscriptionScope;
use sqlx::Connection;

#[tokio::test(start_paused = true)]
async fn failed_hold_write_retries_then_reader_delivers_without_restart() {
    let fixture = OwnerFixture::new().await;
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Wakeable;
    let mut observer = observer(&fixture).await;
    install_pending_write_failure(&mut observer).await;
    let runtime = fixture.runtime().await;
    fixture
        .post(&fixture.root, "Held despite transient storage failure")
        .await;
    await_failed_pending_write(&runtime).await;
    remove_pending_write_failure(&mut observer).await;
    fixture.clock.advance(1).await;
    runtime.synchronize(&fixture.reader).await;
    assert_selection_resolved(&mut observer).await;
    let record = subscription_record(&fixture).await;
    assert_eq!(record.roots()[0].pending_count(), 1);
    assert!(record.roots()[0].held_since().is_some());
    assert!(runtime.requests.lock().await.try_recv().is_err());
    *fixture.presence.0.lock().unwrap() = crate::TargetPresence::Running;
    fixture.clock.advance(30).await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("held since"));
    runtime.await_settled().await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert!(subscription_record(&fixture).await.roots().is_empty());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_retry_mark_retries_then_reader_delivers_without_restart() {
    let fixture = OwnerFixture::new().await;
    let mut observer = observer(&fixture).await;
    install_pending_write_failure(&mut observer).await;
    let runtime = fixture.runtime().await;
    fixture
        .post(
            &fixture.root,
            "Rejected before transient retry write failure",
        )
        .await;
    let first = runtime.requests.lock().await.recv().await.unwrap();
    runtime
        .completions
        .send(DeliveryOutcome::NotSubmitted {
            retryable: false,
            reason: "provider refused this attempt".to_owned(),
        })
        .unwrap();
    await_failed_pending_write(&runtime).await;
    remove_pending_write_failure(&mut observer).await;
    fixture.clock.advance(1).await;
    runtime.synchronize(&fixture.reader).await;
    assert_selection_resolved(&mut observer).await;
    let record = subscription_record(&fixture).await;
    assert_eq!(record.roots()[0].pending_count(), 1);
    assert_eq!(
        record.roots()[0].next_retry_at(),
        Some(fixture.clock.now() + chrono::Duration::seconds(29))
    );
    assert!(runtime.requests.lock().await.try_recv().is_err());
    fixture.clock.advance(29).await;
    let second = runtime.requests.lock().await.recv().await.unwrap();
    assert_ne!(first.payload.push_id, second.payload.push_id);
    runtime.await_settled().await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert!(subscription_record(&fixture).await.roots().is_empty());
    observer.close().await.unwrap();
    runtime.close().await;
}

async fn observer(fixture: &OwnerFixture) -> sqlx::SqliteConnection {
    sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap()
}

async fn install_pending_write_failure(observer: &mut sqlx::SqliteConnection) {
    sqlx::query("CREATE TRIGGER fail_pending_mark BEFORE UPDATE OF in_flight_through ON subscription_windows WHEN OLD.in_flight_through IS NOT NULL AND NEW.in_flight_through IS NULL BEGIN SELECT RAISE(FAIL, 'injected pending mark failure'); END")
        .execute(observer).await.unwrap();
}

async fn remove_pending_write_failure(observer: &mut sqlx::SqliteConnection) {
    sqlx::query("DROP TRIGGER fail_pending_mark")
        .execute(observer)
        .await
        .unwrap();
}

async fn await_failed_pending_write(runtime: &OwnerRuntime) {
    runtime
        .observe(|event| matches!(event, OwnerObservation::Selected))
        .await;
    runtime
        .observe(|event| {
            matches!(
                event,
                OwnerObservation::SettlementRetry | OwnerObservation::Sleeping(_)
            )
        })
        .await;
}

async fn assert_selection_resolved(observer: &mut sqlx::SqliteConnection) {
    let in_flight: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM subscription_windows WHERE in_flight_through IS NOT NULL",
    )
    .fetch_one(observer)
    .await
    .unwrap();
    assert_eq!(
        in_flight, 0,
        "failed pending-state write left the Reader's selection wedged"
    );
}

async fn subscription_record(fixture: &OwnerFixture) -> message_board::ThreadSubscriptionRecord {
    fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap()
        .unwrap()
}
