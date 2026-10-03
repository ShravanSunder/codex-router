use super::*;

#[tokio::test]
async fn restore_clears_interrupted_selection_marks_and_keeps_draining_window_deadline() {
    let mut fixture =
        ThreadSubscriptionFixture::create_without_participant("restore-draining").await;
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Orchestrator,
            true,
            fixture.now,
        )
        .await;
    fixture
        .post_reply_as(
            human("activity-author"),
            "pending before restart",
            fixture.now,
        )
        .await;
    fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: fixture.reader.clone(),
                acting_for: None,
            },
            fixture.now,
        )
        .await
        .unwrap();
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let before_restart = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert_eq!(before_restart.state(), SubscriptionState::Draining);
    assert_eq!(before_restart.roots().len(), 1);

    let interrupted_through = {
        let mut connection = raw_connection(&fixture.path).await;
        let through: i64 = sqlx::query_scalar(
            "SELECT MAX(activity_sequence) FROM board_activity WHERE root_id=? AND kind='threadMessageCreated'",
        )
        .bind(fixture.root_message_id.as_str())
        .fetch_one(&mut connection)
        .await
        .unwrap();
        let reader_key: String = sqlx::query_scalar(
            "SELECT identity_key FROM board_identities WHERE session_id='reader'",
        )
        .fetch_one(&mut connection)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE subscription_windows SET in_flight_through=?,residual_opened_at=? \
             WHERE reader_key=? AND root_id=?",
        )
        .bind(through)
        .bind(super::super::persisted_time(fixture.now + chrono::Duration::seconds(1)).to_rfc3339())
        .bind(reader_key)
        .bind(fixture.root_message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
        connection.close().await.unwrap();
        through
    };
    assert!(interrupted_through > 0);

    let restored = fixture
        .store
        .restore_active_and_draining(fixture.now + chrono::Duration::seconds(2))
        .await
        .unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].state(), SubscriptionState::Draining);
    assert_eq!(
        restored[0].roots()[0].opened_at(),
        before_restart.roots()[0].opened_at()
    );

    let mut connection = raw_connection(&fixture.path).await;
    let row: (Option<i64>, Option<String>, String) = sqlx::query_as(
        "SELECT in_flight_through,residual_opened_at,opened_at FROM subscription_windows WHERE root_id=?",
    )
    .bind(fixture.root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(row.0, None);
    assert_eq!(row.1, None);
    assert_eq!(
        row.2,
        before_restart.roots()[0]
            .opened_at()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    );
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn restart_before_notice_settlement_reselects_the_same_range() {
    let mut fixture = ThreadSubscriptionFixture::create("notice-replay-after-crash").await;
    fixture
        .post_reply_as(
            human("activity-author"),
            "range survives selection",
            fixture.now,
        )
        .await;
    let selection_time = fixture.now + chrono::Duration::seconds(120);
    let due_before_restart = fixture
        .store
        .due_subscription_roots(&fixture.reader, selection_time)
        .await
        .unwrap();
    let (notice_before_restart, _) = fixture
        .store
        .select_subscription_notice(
            &fixture.reader,
            &due_before_restart,
            selection_time,
            usize::MAX,
        )
        .await
        .unwrap();
    assert_eq!(notice_before_restart.roots.len(), 1);
    assert!(
        fixture
            .store
            .due_subscription_roots(
                &fixture.reader,
                selection_time + chrono::Duration::seconds(1),
            )
            .await
            .unwrap()
            .is_empty(),
        "the first selection holds its roots in flight until settlement or restart"
    );

    fixture
        .store
        .restore_active_and_draining(selection_time + chrono::Duration::seconds(1))
        .await
        .unwrap();
    let due_after_restart = fixture
        .store
        .due_subscription_roots(
            &fixture.reader,
            selection_time + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    assert_eq!(due_after_restart, vec![fixture.root_message_id.clone()]);
    let (notice_after_restart, _) = fixture
        .store
        .select_subscription_notice(
            &fixture.reader,
            &due_after_restart,
            selection_time + chrono::Duration::seconds(1),
            usize::MAX,
        )
        .await
        .unwrap();
    assert_eq!(notice_after_restart.roots, notice_before_restart.roots);
    fixture.finish().await;
}
