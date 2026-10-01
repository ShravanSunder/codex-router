use super::super::{SubscriptionClock, subscription_service::OwnerObservation};
use super::owner_fixture::*;
use collaboration_protocol::{DeliveryOutcome, PushDeliveryState};
use message_board::SubscriptionScope;
use sqlx::Connection;

#[tokio::test(start_paused = true)]
async fn failed_wait_attempt_mark_releases_selection_and_returns_explicit_error() {
    let fixture = OwnerFixture::new().await;
    fixture
        .policy(
            &fixture.root,
            message_board::SubscriptionMode::Poll,
            message_board::WhenIdle::Hold,
            0,
            0,
        )
        .await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_wait_attempt BEFORE UPDATE OF delivery_state ON router_pushes WHEN NEW.delivery_state='attempted' BEGIN SELECT RAISE(FAIL, 'injected wait attempt failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Pending poll range").await;
    let result = runtime
        .service
        .wait(
            fixture.reader.clone(),
            super::super::SubscriptionWaitFilter::All,
            60,
        )
        .await;
    assert!(
        result.is_err(),
        "a failed attempted-state write must be reported to the waiter"
    );
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let board = fixture
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
    assert_eq!(board.roots()[0].pending_count(), 1);
    let due = fixture
        .store
        .lock()
        .await
        .due_subscription_roots(&fixture.reader, fixture.clock.now())
        .await
        .unwrap();
    assert_eq!(
        due,
        vec![fixture.root.clone()],
        "unhanded selection must be released and remain retryable"
    );
    sqlx::query("DROP TRIGGER fail_wait_attempt")
        .execute(&mut observer)
        .await
        .unwrap();
    assert!(matches!(
        runtime
            .service
            .wait(
                fixture.reader.clone(),
                super::super::SubscriptionWaitFilter::All,
                60
            )
            .await
            .unwrap(),
        Some(super::super::SubscriptionWaitResult::Notice { .. })
    ));
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn accepted_push_record_settlement_retries_before_board_settlement_without_redelivery() {
    let fixture = OwnerFixture::new().await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_push_settle BEFORE UPDATE OF delivery_state ON router_pushes WHEN NEW.delivery_state='delivered' BEGIN SELECT RAISE(FAIL, 'injected push settlement failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture.post(&fixture.root, "Accepted").await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    runtime.completions.send(DeliveryOutcome::Started).unwrap();
    runtime
        .observe(|event| matches!(event, OwnerObservation::PushSettlementRetry))
        .await;
    let board = fixture
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
    assert_eq!(board.roots()[0].pending_count(), 1);
    assert!(runtime.requests.lock().await.try_recv().is_err());
    sqlx::query("DROP TRIGGER fail_push_settle")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance(1).await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Settled))
        .await;
    runtime.synchronize(&fixture.reader).await;
    let stored = fixture
        .push_store
        .lock()
        .await
        .get_push_record(&request.payload.push_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.delivery_state, PushDeliveryState::Delivered);
    assert!(runtime.requests.lock().await.try_recv().is_err());
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_record_insert_releases_selection_and_never_calls_layer_zero() {
    let fixture = OwnerFixture::new().await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("CREATE TRIGGER fail_push_insert BEFORE INSERT ON router_pushes BEGIN SELECT RAISE(FAIL, 'injected push insertion failure'); END").execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    fixture
        .post(&fixture.root, "Never pushed without a record")
        .await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM router_pushes")
        .fetch_one(&mut observer)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let roots = fixture
        .store
        .lock()
        .await
        .due_subscription_roots(&fixture.reader, fixture.clock.now())
        .await
        .unwrap();
    assert_eq!(roots, vec![fixture.root.clone()]);
    sqlx::query("DROP TRIGGER fail_push_insert")
        .execute(&mut observer)
        .await
        .unwrap();
    fixture.clock.advance(1).await;
    runtime.requests.lock().await.recv().await.unwrap();
    runtime.await_settled().await;
    observer.close().await.unwrap();
    runtime.close().await;
}
