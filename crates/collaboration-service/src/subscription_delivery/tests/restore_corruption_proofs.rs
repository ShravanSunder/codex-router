//! One corrupt recovery row must not suppress healthy readers at Host startup.
use super::super::SubscriptionClock;
use super::owner_fixture::*;
use message_board::*;
use sqlx::Connection;

#[tokio::test(start_paused = true)]
async fn corrupt_subscription_mode_is_skipped_while_valid_reader_restores_and_delivers() {
    let fixture = OwnerFixture::new().await;
    let corrupt_reader = human("corrupt-reader");
    fixture
        .store
        .lock()
        .await
        .join_thread(
            ThreadJoinRequest {
                root_message_id: fixture.root.clone(),
                actor: corrupt_reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                mode: None,
                when_idle: None,
                replace: None,
                note: None,
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    fixture
        .store
        .lock()
        .await
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: corrupt_reader.clone(),
                scope: SubscriptionScope::thread(fixture.root.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    ..SubscriptionPolicyPatch::default()
                },
            },
            fixture.clock.now(),
        )
        .await
        .unwrap();
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("board.sqlite")),
    )
    .await
    .unwrap();
    fixture
        .post(&fixture.root, "Healthy reader pending at restart")
        .await;
    sqlx::query("UPDATE thread_subscriptions SET mode='corrupt-mode' WHERE reader_key IN (SELECT identity_key FROM board_identities WHERE human_id='corrupt-reader')")
        .execute(&mut observer).await.unwrap();
    let runtime = fixture.runtime().await;
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("1 message"));
    runtime.await_settled().await;
    runtime.synchronize(&fixture.reader).await;
    assert!(runtime.requests.lock().await.try_recv().is_err());
    assert!(
        fixture
            .store
            .lock()
            .await
            .get_thread_subscription_record(
                &fixture.reader,
                &SubscriptionScope::thread(fixture.root.clone())
            )
            .await
            .unwrap()
            .unwrap()
            .roots()
            .is_empty()
    );
    let corrupt = fixture
        .store
        .lock()
        .await
        .get_thread_subscription_record(
            &corrupt_reader,
            &SubscriptionScope::thread(fixture.root.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(corrupt.kind, BoardFailureKind::InvalidRecord);
    assert!(corrupt.message.contains("'mode'"));
    let stored_mode: String = sqlx::query_scalar("SELECT mode FROM thread_subscriptions WHERE reader_key IN (SELECT identity_key FROM board_identities WHERE human_id='corrupt-reader')")
        .fetch_one(&mut observer).await.unwrap();
    assert_eq!(
        stored_mode, "corrupt-mode",
        "corrupt row must not be silently repaired"
    );
    observer.close().await.unwrap();
    runtime.close().await;
}

#[tokio::test(start_paused = true)]
async fn failed_direct_message_recovery_query_does_not_stop_valid_subscription_reader() {
    let fixture = OwnerFixture::new().await;
    fixture
        .post(&fixture.root, "Healthy reader despite DM query failure")
        .await;
    let mut observer = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .filename(fixture.directory.path().join("automation.sqlite")),
    )
    .await
    .unwrap();
    // The connection remains usable; only the DM recovery repository query is unavailable.
    sqlx::query("ALTER TABLE router_pushes RENAME TO temporarily_unavailable_pushes")
        .execute(&mut observer)
        .await
        .unwrap();
    let runtime = fixture.runtime().await;
    sqlx::query("ALTER TABLE temporarily_unavailable_pushes RENAME TO router_pushes")
        .execute(&mut observer)
        .await
        .unwrap();
    // Wake the reader after the query becomes available; no Host/service restart.
    runtime
        .service
        .reconcile_reader(fixture.reader.clone())
        .await
        .unwrap();
    let request = runtime.requests.lock().await.recv().await.unwrap();
    assert!(request.payload.line.as_str().contains("1 message"));
    runtime.await_settled().await;
    observer.close().await.unwrap();
    runtime.close().await;
}
