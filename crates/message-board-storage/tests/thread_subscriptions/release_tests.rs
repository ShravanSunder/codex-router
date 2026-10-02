use super::*;

#[tokio::test]
async fn release_subscription_restores_due_without_advancing_delivered() {
    let mut fixture = ThreadSubscriptionFixture::create("release-due").await;
    fixture.post_reply("writer", "Pending").await;
    let due_at = fixture.now + chrono::Duration::seconds(120);
    let roots = vec![fixture.root_message_id.clone()];
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &roots, due_at, usize::MAX)
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .due_subscription_roots(&fixture.reader, due_at)
            .await
            .unwrap()
            .is_empty()
    );
    fixture
        .store
        .release_subscription_batch(&fixture.reader, &settlement)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .due_subscription_roots(&fixture.reader, due_at)
            .await
            .unwrap(),
        roots
    );
    let record = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(record.roots()[0].pending_count(), 1);
    let mut observer = raw_connection(&fixture.path).await;
    let delivered: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM thread_delivery_positions WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut observer)
            .await
            .unwrap();
    assert_eq!(delivered, 0);
    observer.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn release_subscription_stale_window_or_through_fence_is_a_noop() {
    let mut fixture = ThreadSubscriptionFixture::create("release-stale").await;
    fixture.post_reply("writer", "Pending").await;
    let due_at = fixture.now + chrono::Duration::seconds(120);
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(
            &fixture.reader,
            &[fixture.root_message_id.clone()],
            due_at,
            usize::MAX,
        )
        .await
        .unwrap();
    let mut stale = settlement.clone();
    stale.roots[0].window_id = SubscriptionWindowId::generate();
    fixture
        .store
        .release_subscription_batch(&fixture.reader, &stale)
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .due_subscription_roots(&fixture.reader, due_at)
            .await
            .unwrap()
            .is_empty()
    );
    let mut observer = raw_connection(&fixture.path).await;
    let through = i64::try_from(settlement.roots[0].delivered_through.get()).unwrap();
    sqlx::query("UPDATE subscription_windows SET in_flight_through=? WHERE root_id=?")
        .bind(through + 1)
        .bind(fixture.root_message_id.as_str())
        .execute(&mut observer)
        .await
        .unwrap();
    fixture
        .store
        .release_subscription_batch(&fixture.reader, &settlement)
        .await
        .unwrap();
    let still_fenced: i64 =
        sqlx::query_scalar("SELECT in_flight_through FROM subscription_windows WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut observer)
            .await
            .unwrap();
    assert_eq!(still_fenced, through + 1);
    observer.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn release_subscription_preserves_opened_held_and_retry_facts() {
    for retry in [false, true] {
        let mut fixture = ThreadSubscriptionFixture::create("release-preserve").await;
        fixture.post_reply("writer", "Pending").await;
        let selected_at = fixture.now + chrono::Duration::seconds(120);
        let roots = vec![fixture.root_message_id.clone()];
        let (_, selection) = fixture
            .store
            .select_subscription_notice(&fixture.reader, &roots, selected_at, usize::MAX)
            .await
            .unwrap();
        let outcome = SubscriptionDeliveryOutcome::NotSubmitted {
            retryable: true,
            reason: "hold".to_owned(),
        };
        if retry {
            fixture
                .store
                .mark_subscription_batch_retry(
                    &fixture.reader,
                    &selection,
                    selected_at,
                    selected_at + chrono::Duration::seconds(30),
                    outcome,
                )
                .await
                .unwrap();
        } else {
            fixture
                .store
                .mark_subscription_batch_held(&fixture.reader, &selection, selected_at, outcome)
                .await
                .unwrap();
        }
        let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
        let before = record(&mut fixture.store, &fixture.reader, &scope).await;
        let (_, selection) = fixture
            .store
            .select_subscription_notice(
                &fixture.reader,
                &roots,
                selected_at + chrono::Duration::seconds(30),
                usize::MAX,
            )
            .await
            .unwrap();
        fixture
            .post_reply_as(
                human("writer"),
                "Arrives in flight",
                selected_at + chrono::Duration::seconds(31),
            )
            .await;
        fixture
            .store
            .release_subscription_batch(&fixture.reader, &selection)
            .await
            .unwrap();
        let after = record(&mut fixture.store, &fixture.reader, &scope).await;
        assert_eq!(after.roots()[0].opened_at(), before.roots()[0].opened_at());
        assert_eq!(
            after.roots()[0].held_since(),
            before.roots()[0].held_since()
        );
        assert_eq!(
            after.roots()[0].next_retry_at(),
            before.roots()[0].next_retry_at()
        );
        assert_eq!(
            after.roots()[0].retry_attempts(),
            before.roots()[0].retry_attempts()
        );
        assert_eq!(after.last_outcome(), before.last_outcome());
        assert_eq!(after.roots()[0].pending_count(), 2);
        let mut observer = raw_connection(&fixture.path).await;
        let residual: Option<String> = sqlx::query_scalar(
            "SELECT residual_opened_at FROM subscription_windows WHERE root_id=?",
        )
        .bind(fixture.root_message_id.as_str())
        .fetch_one(&mut observer)
        .await
        .unwrap();
        assert_eq!(residual, None);
        observer.close().await.unwrap();
        fixture.finish().await;
    }
}
