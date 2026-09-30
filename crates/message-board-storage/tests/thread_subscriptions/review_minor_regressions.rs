use super::*;
use sqlx::{Connection, SqliteConnection};

async fn raw_connection(path: &std::path::Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap()
}

#[tokio::test]
async fn m2_corrupt_ended_row_reports_the_missing_ended_at_field() {
    let mut fixture = ThreadSubscriptionFixture::create("review-m2").await;
    fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root_message_id.clone()),
            },
            fixture.now,
        )
        .await
        .unwrap();

    let mut connection = raw_connection(&fixture.path).await;
    sqlx::query("PRAGMA ignore_check_constraints=ON")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE thread_subscriptions SET ended_at=NULL \
         WHERE scope_kind='thread' AND scope_id=?",
    )
    .bind(fixture.root_message_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let error = fixture
        .store
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root_message_id.clone()),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.details,
        BoardErrorDetails::FieldConstraint { ref field, .. }
            if field == "threadSubscription.endedAt"
    ));
    fixture.finish().await;
}

#[tokio::test]
async fn m3_unsubscribe_after_expiry_keeps_the_expired_end_reason() {
    let mut fixture = ThreadSubscriptionFixture::create("review-m3").await;
    let expired_at = fixture.now + chrono::Duration::hours(24) + chrono::Duration::seconds(1);
    let record = fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root_message_id.clone()),
            },
            expired_at,
        )
        .await
        .unwrap();
    assert_eq!(
        record.state,
        SubscriptionState::Ended {
            reason: EndReason::Expired
        }
    );
    fixture.finish().await;
}

#[tokio::test]
async fn m6_draining_thread_subscription_completes_as_resolved_after_settlement() {
    let mut fixture = ThreadSubscriptionFixture::create("review-m6").await;
    fixture
        .post_reply_as(
            human("review-m6-author"),
            "pending while the thread resolves",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: human("owner"),
                acting_for: None,
            },
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();

    let too_early = fixture
        .store
        .complete_thread_subscription_drain(
            &fixture.reader,
            &fixture.root_message_id,
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;
    assert!(too_early.is_err());

    let due_at = fixture.now + chrono::Duration::seconds(122);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, due_at)
        .await
        .unwrap();
    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();

    let completed = fixture
        .store
        .complete_thread_subscription_drain(
            &fixture.reader,
            &fixture.root_message_id,
            due_at + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    assert_eq!(
        completed.state,
        SubscriptionState::Ended {
            reason: EndReason::Resolved
        }
    );
    assert!(completed.roots.is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn m7_rescan_prunes_a_window_with_no_pending_messages() {
    let mut fixture = ThreadSubscriptionFixture::create("review-m7").await;
    let reply = fixture
        .post_reply_as(
            human("review-m7-author"),
            "the window will become empty",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let mut connection = raw_connection(&fixture.path).await;
    let reader_key: String =
        sqlx::query_scalar("SELECT identity_key FROM board_identities WHERE session_id='reader'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through) \
         VALUES(?,?,?) ON CONFLICT(reader_key,root_id) DO UPDATE SET \
         delivered_through=excluded.delivered_through",
    )
    .bind(reader_key)
    .bind(fixture.root_message_id.as_str())
    .bind(i64::try_from(reply.message.activity_sequence.get()).unwrap())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    fixture
        .store
        .rescan_missing_subscription_windows(
            &fixture.reader,
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();
    let record = fixture
        .store
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root_message_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(record.roots.is_empty());
    fixture.finish().await;
}
