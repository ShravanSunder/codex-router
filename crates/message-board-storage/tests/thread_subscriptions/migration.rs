use super::*;
use sqlx::{Connection, Row, SqliteConnection};

async fn raw_connection(path: &std::path::Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap()
}

async fn reader_key_for_session(connection: &mut SqliteConnection, session_id: &str) -> String {
    sqlx::query_scalar("SELECT identity_key FROM board_identities WHERE session_id=?")
        .bind(session_id)
        .fetch_one(connection)
        .await
        .unwrap()
}

async fn subscription(
    store: &mut BoardStore,
    reader: &Identity,
    root_message_id: &MessageId,
) -> Option<ThreadSubscriptionRecord> {
    store
        .get_thread_subscription_record(reader, &SubscriptionScope::thread(root_message_id.clone()))
        .await
        .unwrap()
}

#[tokio::test]
async fn subscription_window_schema_has_no_overflow_column() {
    let fixture = ThreadSubscriptionFixture::create("notice-window-schema").await;
    let mut connection = raw_connection(&fixture.path).await;
    let rows = sqlx::query("PRAGMA table_info(subscription_windows)")
        .fetch_all(&mut connection)
        .await
        .unwrap();
    let columns = rows
        .iter()
        .map(|row| row.get::<String, _>("name"))
        .collect::<Vec<_>>();
    assert!(!columns.iter().any(|column| column == "overflow"));
    connection.close().await.unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn backfill_requires_open_session_unresolved_thread_and_active_watch() {
    let mut fixture = ThreadSubscriptionFixture::create("backfill-eligible").await;
    let old_delivered_reader = session("old-delivered");
    fixture
        .join_reader(
            old_delivered_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;
    let human_reader = human("human-participant");
    fixture
        .join_reader(
            human_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;
    let no_watch_reader = session("no-watch-participant");
    fixture
        .join_reader(
            no_watch_reader.clone(),
            ParticipantRole::Participant,
            false,
            fixture.now,
        )
        .await;
    let closed_reader = session("closed-participant");
    fixture
        .join_reader(
            closed_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;

    let resolved_root = fixture
        .create_root(
            human("root-author"),
            "resolved root",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let resolved_reader = session("resolved-participant");
    fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                root_message_id: resolved_root.clone(),
                actor: resolved_reader.clone(),
                role: ParticipantRole::Participant,
                watch: true,
                replace: None,
                note: None,
            },
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("activity-author"),
            "latest activity for backfill",
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;

    let path = fixture.path.clone();
    let root_message_id = fixture.root_message_id.clone();
    fixture.store.close().await.unwrap();
    let mut connection = raw_connection(&path).await;
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    let root_activity: i64 =
        sqlx::query_scalar("SELECT activity_sequence FROM board_activity WHERE message_id=?")
            .bind(root_message_id.as_str())
            .fetch_one(&mut connection)
            .await
            .unwrap();
    let latest_activity: i64 = sqlx::query_scalar(
        "SELECT MAX(activity_sequence) FROM board_activity \
         WHERE (root_id=? AND kind='threadMessageCreated') \
           OR (message_id=? AND kind='mainMessageCreated')",
    )
    .bind(root_message_id.as_str())
    .bind(root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert!(latest_activity > root_activity);
    let old_delivered_key = reader_key_for_session(&mut connection, "old-delivered").await;
    sqlx::query(
        "INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through) VALUES(?,?,?)",
    )
    .bind(old_delivered_key)
    .bind(root_message_id.as_str())
    .bind(root_activity)
    .execute(&mut connection)
    .await
    .unwrap();
    let closed_reader_key = reader_key_for_session(&mut connection, "closed-participant").await;
    sqlx::query(
        "UPDATE thread_participants SET closed_at_activity=joined_at_activity,closed_reason='left' \
         WHERE reader_key=? AND root_id=?",
    )
    .bind(closed_reader_key)
    .bind(root_message_id.as_str())
    .execute(&mut connection)
    .await
    .unwrap();
    let no_watch_key = reader_key_for_session(&mut connection, "no-watch-participant").await;
    sqlx::query("DELETE FROM thread_watches WHERE reader_key=? AND root_id=?")
        .bind(no_watch_key)
        .bind(root_message_id.as_str())
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE board_threads SET state='resolved' WHERE root_id=?")
        .bind(resolved_root.as_str())
        .execute(&mut connection)
        .await
        .unwrap();

    sqlx::query("DROP TABLE subscription_windows")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DROP TABLE thread_subscriptions")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=202609170001")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();

    let mut migrated = BoardStore::open(&path).await.unwrap();
    let eligible = subscription(&mut migrated, &fixture.reader, &root_message_id)
        .await
        .unwrap();
    let eligible_with_old_delivered =
        subscription(&mut migrated, &old_delivered_reader, &root_message_id)
            .await
            .unwrap();
    for record in [&eligible, &eligible_with_old_delivered] {
        assert_eq!(record.state, SubscriptionState::Active);
        assert_eq!(
            record.policy,
            SubscriptionPolicy::defaults_for(&record.reader)
        );
        assert_eq!(record.roots.len(), 0);
        assert_eq!(
            record.expires_at.signed_duration_since(record.renewed_at),
            chrono::Duration::hours(24)
        );
    }
    assert!(
        subscription(&mut migrated, &human_reader, &root_message_id)
            .await
            .is_none()
    );
    assert!(
        subscription(&mut migrated, &no_watch_reader, &root_message_id)
            .await
            .is_none()
    );
    assert!(
        subscription(&mut migrated, &closed_reader, &root_message_id)
            .await
            .is_none()
    );
    assert!(
        subscription(&mut migrated, &resolved_reader, &resolved_root)
            .await
            .is_none()
    );
    migrated.close().await.unwrap();

    let mut connection = raw_connection(&path).await;
    let eligible_key = reader_key_for_session(&mut connection, "reader").await;
    let delivered_without_prior_position: i64 = sqlx::query_scalar(
        "SELECT delivered_through FROM thread_delivery_positions WHERE reader_key=? AND root_id=?",
    )
    .bind(eligible_key)
    .bind(root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let old_position_after_backfill: i64 = sqlx::query_scalar(
        "SELECT delivered_through FROM thread_delivery_positions WHERE reader_key=? AND root_id=?",
    )
    .bind(reader_key_for_session(&mut connection, "old-delivered").await)
    .bind(root_message_id.as_str())
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(delivered_without_prior_position, latest_activity);
    assert_eq!(old_position_after_backfill, latest_activity);
    let subscription_count: i64 = sqlx::query_scalar("SELECT count(*) FROM thread_subscriptions")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(subscription_count, 2);
    connection.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn p1_backfill_delivered_stays_at_message_activity_when_join_is_latest() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("review-p1").await;
    fixture
        .post_reply_as(
            human("review-p1-author"),
            "reply before participant joined",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;

    let path = fixture.path.clone();
    let reader = fixture.reader.clone();
    let project_id = fixture.project_id.clone();
    fixture.store.close().await.unwrap();
    let mut connection = raw_connection(&path).await;
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM thread_delivery_positions")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DROP TABLE subscription_windows")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DROP TABLE thread_subscriptions")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=202609170001")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();

    fixture.store = BoardStore::open(&path).await.unwrap();
    fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project { project_id },
            reader: reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest {
                limit: 100_u32.try_into().unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    let own_post = fixture
        .post_reply_as(
            reader.clone(),
            "reader post after upgrade",
            fixture.now + chrono::Duration::seconds(5),
        )
        .await;
    let other_post = fixture
        .post_reply_as(
            human("review-p1-author"),
            "other post after upgrade",
            fixture.now + chrono::Duration::seconds(6),
        )
        .await;
    let due = fixture
        .store
        .due_subscription_roots(&reader, fixture.now + chrono::Duration::seconds(200))
        .await
        .unwrap();
    assert_eq!(due, vec![fixture.root_message_id.clone()]);
    let (notice, _) = fixture
        .store
        .select_subscription_notice(&reader, &due, fixture.now + chrono::Duration::seconds(200))
        .await
        .unwrap();
    assert_eq!(notice.roots[0].message_count, 1);
    assert_eq!(
        notice.roots[0].from_sequence,
        other_post.message.activity_sequence
    );
    assert_eq!(
        notice.roots[0].through_sequence,
        other_post.message.activity_sequence
    );
    assert!(own_post.message.activity_sequence < other_post.message.activity_sequence);
    fixture.finish().await;
}
