use super::super::{SubscriptionClock, subscription_service::OwnerObservation};
use super::owner_fixture::*;
use sqlx::Connection;
use std::sync::Arc;

#[tokio::test(start_paused = true)]
async fn timeout_free_original_elapsed_clock_reaches_due_selection_and_prepared_delivery() {
    let fixture = OwnerFixture::new().await;
    let clock = Arc::new(ElapsedClock {
        wall_start: fixture.clock.now(),
        monotonic_start: tokio::time::Instant::now(),
    });
    let runtime = fixture.runtime_with_clock(clock.clone()).await;
    fixture
        .post_at(&fixture.root, "Timeout-free original clock", clock.now())
        .await;
    let mut observed = Vec::new();
    {
        let mut events = runtime.observations.lock().await;
        loop {
            let event = events.recv().await.unwrap();
            let due = matches!(event, OwnerObservation::DueRoots(count) if count > 0);
            observed.push(event);
            if due {
                break;
            }
        }
    }
    assert!(
        observed
            .iter()
            .any(|event| matches!(event, OwnerObservation::RowsLoaded(1)))
    );
    println!("timeout-free owner observations: {observed:?}");
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(
        request
            .payload
            .line
            .as_str()
            .starts_with("🧵 Router: new thread activity")
    );
    runtime.await_settled().await;
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn virtual_timeout_can_expire_while_a_real_sqlite_due_query_remains_in_progress() {
    let fixture = OwnerFixture::new().await;
    fixture.post(&fixture.root, "Due before query").await;
    let mut blocker = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap();
    sqlx::query("BEGIN EXCLUSIVE")
        .execute(&mut blocker)
        .await
        .unwrap();
    let store = fixture.store.clone();
    let reader = fixture.reader.clone();
    let now = fixture.clock.now();
    let (started, started_receiver) = tokio::sync::oneshot::channel();
    let mut query = tokio::spawn(async move {
        let mut store = store.lock().await;
        started.send(()).unwrap();
        store.due_subscription_roots(&reader, now).await
    });
    started_receiver.await.unwrap();
    let before = tokio::time::Instant::now();
    let timeout = tokio::time::timeout(std::time::Duration::from_secs(1), &mut query).await;
    assert!(timeout.is_err());
    assert!(!query.is_finished());
    println!(
        "virtual timer elapsed {:?} while SQLite query was still pending",
        tokio::time::Instant::now().duration_since(before)
    );
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    assert_eq!(query.await.unwrap().unwrap(), vec![fixture.root.clone()]);
    blocker.close().await.unwrap();
}
