use super::*;

#[tokio::test]
async fn held_mark_persists_hold_time_without_advancing_delivered() {
    let mut fixture = ThreadSubscriptionFixture::create("held-mark").await;
    let posted = fixture
        .post_reply_as(human("sender"), "held activity", fixture.now)
        .await;
    let mut connection = raw_connection(&fixture.path).await;
    let window_id: String =
        sqlx::query_scalar("SELECT window_id FROM subscription_windows WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let reader_key: String =
        sqlx::query_scalar("SELECT identity_key FROM board_identities WHERE session_id='reader'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    sqlx::query(
        "UPDATE subscription_windows SET in_flight_through=? WHERE reader_key=? AND root_id=?",
    )
    .bind(i64::try_from(posted.message.activity_sequence.get()).unwrap())
    .bind(reader_key)
    .bind(fixture.root_message_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let initial_record = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    let held_at = fixture.now + chrono::Duration::seconds(1);
    fixture
        .store
        .mark_subscription_batch_held(
            &fixture.reader,
            &SubscriptionBatchSettlement {
                roots: vec![SubscriptionBatchRootSettlement {
                    root_message_id: fixture.root_message_id.clone(),
                    window_id: SubscriptionWindowId::try_from(window_id).unwrap(),
                    delivered_through: posted.message.activity_sequence,
                    subscription_scope: SubscriptionScope::thread(fixture.root_message_id.clone()),
                    subscription_generation: initial_record.generation(),
                }],
            },
            held_at,
            SubscriptionDeliveryOutcome::NotSubmitted {
                reason: "targetNotRunning".to_owned(),
                retryable: true,
            },
        )
        .await
        .unwrap();

    let held = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(held.roots()[0].held_since(), Some(persisted_time(held_at)));
    assert_eq!(held.roots()[0].pending_count(), 1);
    assert!(matches!(
        held.last_outcome(),
        Some(SubscriptionDeliveryOutcome::NotSubmitted {
            retryable: true,
            ..
        })
    ));
    let mut connection = raw_connection(&fixture.path).await;
    let delivered_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_delivery_positions WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(delivered_rows, 0);
    let still_fenced: Option<i64> =
        sqlx::query_scalar("SELECT in_flight_through FROM subscription_windows WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(still_fenced, None);
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn arrivals_during_hold_do_not_restart_the_original_window_cap() {
    let mut fixture = ThreadSubscriptionFixture::create("held-window-cap").await;
    let first_arrival_at = fixture.now + chrono::Duration::seconds(1);
    fixture
        .post_reply_as(
            human("hold-cap-author"),
            "opens the window",
            first_arrival_at,
        )
        .await;

    let selection_at = fixture.now + chrono::Duration::seconds(122);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, selection_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, selection_at)
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("hold-cap-author"),
            "arrives while selection is in flight",
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;
    fixture
        .store
        .mark_subscription_batch_held(
            &fixture.reader,
            &settlement,
            fixture.now + chrono::Duration::seconds(3),
            SubscriptionDeliveryOutcome::NotSubmitted {
                reason: "targetNotRunning".to_owned(),
                retryable: true,
            },
        )
        .await
        .unwrap();

    let held_record = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(
        held_record.roots()[0].opened_at(),
        persisted_time(first_arrival_at),
        "marking an in-flight window held must preserve its original opened_at"
    );

    fixture
        .post_reply_as(
            human("hold-cap-author"),
            "continuous activity while held",
            fixture.now + chrono::Duration::seconds(550),
        )
        .await;
    let cap_deadline = first_arrival_at + chrono::Duration::seconds(600);
    let due_at_cap = fixture
        .store
        .due_subscription_roots(&fixture.reader, cap_deadline)
        .await
        .unwrap();
    assert_eq!(
        due_at_cap,
        vec![fixture.root_message_id.clone()],
        "the original cap must make the held root due despite continuous arrivals"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn drop_storage_primitive_does_not_skip_hold_policy_activity() {
    let mut fixture = ThreadSubscriptionFixture::create("drop-hold-policy").await;
    fixture
        .post_reply_as(
            human("sender"),
            "must remain pending under hold",
            fixture.now,
        )
        .await;
    fixture
        .store
        .drop_subscription_roots(
            &fixture.reader,
            std::slice::from_ref(&fixture.root_message_id),
            fixture.now + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    let subscription = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(subscription.roots().len(), 1);
    assert_eq!(subscription.roots()[0].pending_count(), 1);
    let mut connection = raw_connection(&fixture.path).await;
    let delivered_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_delivery_positions WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(delivered_rows, 0);
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn expiration_does_not_normalize_a_corrupt_subscription_row() {
    let mut fixture = ThreadSubscriptionFixture::create("corrupt-expiry").await;
    let mut connection = raw_connection(&fixture.path).await;
    sqlx::query(
        "UPDATE thread_subscriptions SET mode='unknown-mode',expires_at='2000-01-01T00:00:00.000Z'",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let error = fixture
        .store
        .end_expired_subscriptions(fixture.now)
        .await
        .unwrap_err();
    assert!(error.message.contains("mode"));

    let mut connection = raw_connection(&fixture.path).await;
    let row: (String, String) =
        sqlx::query_as("SELECT mode,state FROM thread_subscriptions WHERE scope_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(row, ("unknown-mode".to_owned(), "active".to_owned()));
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn late_lower_settlement_never_moves_delivered_backwards() {
    let mut fixture = ThreadSubscriptionFixture::create("late-lower-settlement").await;
    let first = fixture
        .post_reply_as(human("first-author"), "first", fixture.now)
        .await;
    let second = fixture
        .post_reply_as(
            human("second-author"),
            "second",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let selection_time = fixture.now + chrono::Duration::seconds(121);
    let due_roots = fixture
        .store
        .due_subscription_roots(&fixture.reader, selection_time)
        .await
        .unwrap();
    let (_, mut settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due_roots, selection_time)
        .await
        .unwrap();
    settlement.roots[0].delivered_through = first.message.activity_sequence;

    let listen_context = fixture
        .store
        .prepare_thread_listen(&fixture.listen_request())
        .await
        .unwrap();
    let listen_batch = fixture
        .store
        .select_pending_thread_listen_batch_set(ListenId::generate(), &listen_context, 32_000)
        .await
        .unwrap();
    assert_eq!(
        listen_batch.batches[0].delivered_through,
        second.message.activity_sequence
    );
    fixture
        .store
        .record_thread_listen_batch_delivery(&listen_context, &listen_batch)
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

    let pending = fixture
        .store
        .select_pending_thread_listen_batch_set(ListenId::generate(), &listen_context, 32_000)
        .await
        .unwrap();
    assert!(pending.batches.is_empty());
    let mut connection = raw_connection(&fixture.path).await;
    let delivered: i64 = sqlx::query_scalar(
        "SELECT delivered_through FROM thread_delivery_positions WHERE root_id=?",
    )
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        delivered,
        i64::try_from(second.message.activity_sequence.get()).unwrap()
    );
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn old_settlement_after_cancel_and_resubscribe_preserves_replacement_window_and_outcome() {
    let mut fixture = ThreadSubscriptionFixture::create("late-fenced-settlement").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let old_post = fixture
        .post_reply_as(human("first-author"), "old batch", fixture.now)
        .await;
    let old_selection_time = fixture.now + chrono::Duration::seconds(120);
    let old_due = fixture
        .store
        .due_subscription_roots(&fixture.reader, old_selection_time)
        .await
        .unwrap();
    let (_, old_settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &old_due, old_selection_time)
        .await
        .unwrap();

    fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
            },
            fixture.now + chrono::Duration::seconds(121),
        )
        .await
        .unwrap();
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now + chrono::Duration::seconds(122),
        )
        .await
        .unwrap();
    let replacement_post = fixture
        .post_reply_as(
            human("second-author"),
            "replacement batch",
            fixture.now + chrono::Duration::seconds(123),
        )
        .await;
    let replacement_select_time = fixture.now + chrono::Duration::seconds(243);
    let replacement_due = fixture
        .store
        .due_subscription_roots(&fixture.reader, replacement_select_time)
        .await
        .unwrap();
    let (_, replacement_settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &replacement_due, replacement_select_time)
        .await
        .unwrap();
    let retry_at = replacement_select_time + chrono::Duration::seconds(30);
    fixture
        .store
        .mark_subscription_batch_retry(
            &fixture.reader,
            &replacement_settlement,
            replacement_select_time,
            retry_at,
            SubscriptionDeliveryOutcome::Rejected {
                evidence: serde_json::json!({"receipt": "replacement"}),
            },
        )
        .await
        .unwrap();
    let before_old_settlement = record(&mut fixture.store, &fixture.reader, &scope).await;
    let mut connection = raw_connection(&fixture.path).await;
    let window_id_before: String = sqlx::query_scalar(
        "SELECT window_id FROM subscription_windows WHERE reader_key=(SELECT identity_key FROM board_identities WHERE session_id=?) AND root_id=?",
    )
    .bind("reader")
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &old_settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();
    let after_old_settlement = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert_eq!(
        after_old_settlement.roots().len(),
        before_old_settlement.roots().len()
    );
    assert_eq!(
        after_old_settlement.roots()[0].root_message_id(),
        before_old_settlement.roots()[0].root_message_id()
    );
    assert_eq!(
        after_old_settlement.roots()[0].opened_at(),
        before_old_settlement.roots()[0].opened_at()
    );
    assert_eq!(
        after_old_settlement.roots()[0].last_arrival_at(),
        before_old_settlement.roots()[0].last_arrival_at()
    );
    assert_eq!(after_old_settlement.roots()[0].pending_count(), 1);
    assert_eq!(
        after_old_settlement.roots()[0].held_since(),
        before_old_settlement.roots()[0].held_since()
    );
    assert_eq!(
        after_old_settlement.roots()[0].next_retry_at(),
        before_old_settlement.roots()[0].next_retry_at()
    );
    assert_eq!(
        after_old_settlement.last_outcome(),
        before_old_settlement.last_outcome()
    );
    assert!(matches!(
        after_old_settlement.last_outcome(),
        Some(SubscriptionDeliveryOutcome::Rejected { .. })
    ));
    assert_eq!(
        after_old_settlement.roots()[0].next_retry_at(),
        Some(persisted_time(retry_at))
    );

    let mut connection = raw_connection(&fixture.path).await;
    let window_id_after: String = sqlx::query_scalar(
        "SELECT window_id FROM subscription_windows WHERE reader_key=(SELECT identity_key FROM board_identities WHERE session_id=?) AND root_id=?",
    )
    .bind("reader")
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let delivered: i64 = sqlx::query_scalar(
        "SELECT delivered_through FROM thread_delivery_positions WHERE root_id=?",
    )
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(window_id_after, window_id_before);
    assert_eq!(
        delivered,
        i64::try_from(old_post.message.activity_sequence.get()).unwrap()
    );
    assert!(replacement_post.message.activity_sequence > old_post.message.activity_sequence);
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn settlement_failure_rolls_back_delivered_window_and_outcome_together() {
    let mut fixture = ThreadSubscriptionFixture::create("settlement-atomicity").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let posted = fixture
        .post_reply_as(human("sender"), "atomic delivery", fixture.now)
        .await;
    let due_at = fixture.now + chrono::Duration::seconds(120);
    let due_roots = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due_roots, due_at)
        .await
        .unwrap();
    let before = record(&mut fixture.store, &fixture.reader, &scope).await;
    let mut connection = raw_connection(&fixture.path).await;
    sqlx::query(
        "CREATE TRIGGER fail_subscription_outcome BEFORE UPDATE OF last_outcome ON thread_subscriptions BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    assert!(
        fixture
            .store
            .settle_subscription_batch(
                &fixture.reader,
                &settlement,
                SubscriptionDeliveryOutcome::Accepted,
            )
            .await
            .is_err()
    );
    let after_failure = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert_eq!(after_failure.roots(), before.roots());
    assert_eq!(after_failure.last_outcome(), None);
    let mut connection = raw_connection(&fixture.path).await;
    let delivered_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_delivery_positions WHERE root_id=?")
            .bind(fixture.root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(delivered_count, 0);
    sqlx::query("DROP TRIGGER fail_subscription_outcome")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();

    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();
    let settled = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert!(settled.roots().is_empty());
    assert_eq!(
        settled.last_outcome().cloned(),
        Some(SubscriptionDeliveryOutcome::Accepted)
    );
    let mut connection = raw_connection(&fixture.path).await;
    let delivered: i64 = sqlx::query_scalar(
        "SELECT delivered_through FROM thread_delivery_positions WHERE root_id=?",
    )
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        delivered,
        i64::try_from(posted.message.activity_sequence.get()).unwrap()
    );
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn drop_advances_delivered_but_keeps_activity_unread() {
    let mut fixture = ThreadSubscriptionFixture::create("drop-unread").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: policy_patch(
                    Some(SubscriptionMode::Deliver),
                    Some(WhenIdle::Drop),
                    None,
                    None,
                    None,
                ),
            },
            fixture.now,
        )
        .await
        .unwrap();
    let skipped = fixture
        .post_reply_as(
            human("sender"),
            "skipped while idle",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    fixture
        .store
        .drop_subscription_roots(
            &fixture.reader,
            std::slice::from_ref(&fixture.root_message_id),
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();
    let after_drop = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert!(after_drop.roots().is_empty());
    let unread = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader: fixture.reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest {
                limit: PageLimit::try_from(100).unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap()
        .page
        .records;
    assert!(unread.iter().any(|activity| matches!(
        activity,
        InboxActivity::MessageCreated { message, .. }
            if message.message_id == skipped.message.message_id
    )));
    fixture.finish().await;
}

#[tokio::test]
async fn corrupted_subscription_enum_fails_closed_with_field_name() {
    let mut fixture = ThreadSubscriptionFixture::create("corrupt-enum").await;
    let mut connection = raw_connection(&fixture.path).await;
    sqlx::query("UPDATE thread_subscriptions SET mode='unknown-mode'")
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
    assert!(error.message.contains("mode"));
    fixture.finish().await;
}

#[tokio::test]
async fn rescan_reopens_a_missing_window_from_pending_activity() {
    let mut fixture = ThreadSubscriptionFixture::create("rescan-missing-window").await;
    fixture
        .post_reply_as(human("sender"), "pending without window", fixture.now)
        .await;
    let mut connection = raw_connection(&fixture.path).await;
    sqlx::query("DELETE FROM subscription_windows")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert_eq!(
        fixture
            .store
            .rescan_missing_subscription_windows(&fixture.reader, fixture.now)
            .await
            .unwrap(),
        1
    );
    let record = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(record.roots().len(), 1);
    assert_eq!(record.roots()[0].opened_at(), persisted_time(fixture.now));
    fixture.finish().await;
}
