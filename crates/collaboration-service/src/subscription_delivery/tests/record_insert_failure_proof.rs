//! A failed durable push insertion must release selection without invoking delivery.
use super::*;

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
    runtime
        .observe(|event| matches!(event, OwnerObservation::Sleeping(_)))
        .await;
    fixture
        .post(&fixture.root, "Never pushed without a record")
        .await;
    // Consume the single post notification and observe the completed error path;
    // a command barrier would itself permit a new selection before inspection.
    runtime
        .observe(|event| matches!(event, OwnerObservation::Selected))
        .await;
    runtime
        .observe(|event| matches!(event, OwnerObservation::Sleeping(Some(_))))
        .await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM router_pushes")
        .fetch_one(&mut observer)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let mut board_observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap();
    let in_flight: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM subscription_windows WHERE in_flight_through IS NOT NULL",
    )
    .fetch_one(&mut board_observer)
    .await
    .unwrap();
    assert_eq!(
        in_flight, 0,
        "failed insertion must release its selected window before retry wait"
    );
    board_observer.close().await.unwrap();
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
