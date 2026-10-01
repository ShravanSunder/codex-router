use super::*;
use sqlx::{Connection, SqliteConnection};
#[path = "release_tests.rs"]
mod release_tests;

fn policy_patch(
    mode: Option<SubscriptionMode>,
    when_idle: Option<WhenIdle>,
    quiet_seconds: Option<u64>,
    cap_seconds: Option<u64>,
    lifetime_seconds: Option<u64>,
) -> SubscriptionPolicyPatch {
    SubscriptionPolicyPatch {
        mode,
        when_idle,
        timing: SubscriptionTimingPatch {
            quiet_seconds,
            cap_seconds,
        },
        lifetime_seconds,
    }
}

async fn record(
    store: &mut BoardStore,
    reader: &Identity,
    scope: &SubscriptionScope,
) -> ThreadSubscriptionRecord {
    store
        .get_thread_subscription_record(reader, scope)
        .await
        .unwrap()
        .unwrap()
}

async fn raw_connection(path: &std::path::Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap()
}

#[tokio::test]
async fn subscription_notice_returns_only_ranges_and_counts_and_keeps_messages_unread() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("root-notice").await;
    let root_content = format!("  {}  \nignored second line", "é".repeat(90));
    let root_message_id = fixture
        .create_root(
            human("root-author"),
            &root_content,
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let reader = session("notice-reader");
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();

    let first_author = human("first-author");
    let second_author = human("second-author");
    let first = fixture
        .post(
            Placement::Thread {
                root_message_id: root_message_id.clone(),
            },
            first_author.clone(),
            "first private body",
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;
    fixture
        .post(
            Placement::Thread {
                root_message_id: root_message_id.clone(),
            },
            second_author.clone(),
            "second private body",
            fixture.now + chrono::Duration::seconds(4),
        )
        .await;
    let last = fixture
        .post(
            Placement::Thread {
                root_message_id: root_message_id.clone(),
            },
            first_author.clone(),
            "third private body",
            fixture.now + chrono::Duration::seconds(5),
        )
        .await;

    let due_at = fixture.now + chrono::Duration::seconds(125);
    let due_roots = fixture
        .store
        .due_subscription_roots(&reader, due_at)
        .await
        .unwrap();
    assert_eq!(due_roots, vec![root_message_id.clone()]);
    let (notice, settlement) = fixture
        .store
        .select_subscription_notice(&reader, &due_roots, due_at)
        .await
        .unwrap();

    assert_eq!(notice.roots.len(), 1);
    let root_notice = &notice.roots[0];
    assert_eq!(root_notice.root_id, root_message_id);
    assert_eq!(root_notice.topic_id, fixture.topic_id);
    assert_eq!(root_notice.from_sequence, first.message.activity_sequence);
    assert_eq!(root_notice.through_sequence, last.message.activity_sequence);
    assert_eq!(root_notice.message_count, 3);
    let encoded_notice = serde_json::to_string(&notice).unwrap();
    assert!(!encoded_notice.contains("first private body"));
    assert!(!encoded_notice.contains("second private body"));
    assert!(!encoded_notice.contains("third private body"));
    assert!(!encoded_notice.contains(&root_content));
    assert!(!encoded_notice.contains("first-author"));
    assert!(!encoded_notice.contains("second-author"));
    assert!(!encoded_notice.contains("authors"));
    assert!(!encoded_notice.contains("rootTitleExcerpt"));

    let fetched = fixture
        .store
        .list_messages(MessageListRequest {
            scope: MessageListScope::Thread {
                root_message_id: root_message_id.clone(),
            },
            selection: MessageSelection::Range {
                from_activity_sequence: root_notice.from_sequence,
                to_activity_sequence: root_notice.through_sequence,
            },
            page: PageRequest {
                limit: 100_u32.try_into().unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(fetched.page.records.len(), 3);
    assert_eq!(fetched.page.records[0].message_id, first.message.message_id);

    fixture
        .store
        .settle_subscription_batch(&reader, &settlement, SubscriptionDeliveryOutcome::Accepted)
        .await
        .unwrap();
    let unread = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader,
            read_mode: InboxReadMode::Unread,
            page: PageRequest {
                limit: 100_u32.try_into().unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(unread.page.records.len(), 3);
    fixture.finish().await;
}

#[tokio::test]
async fn subscription_notice_caps_roots_without_settling_omitted_due_windows() {
    let mut fixture =
        ThreadSubscriptionFixture::create_without_participant("notice-root-cap").await;
    let reader = session("notice-cap-reader");
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    for root_index in 0..21 {
        fixture
            .create_root(
                human(&format!("root-author-{root_index}")),
                &format!("notice root {root_index}"),
                fixture.now + chrono::Duration::seconds(i64::from(root_index + 1)),
            )
            .await;
    }
    let due_at = fixture.now + chrono::Duration::seconds(200);
    let due_roots = fixture
        .store
        .due_subscription_roots(&reader, due_at)
        .await
        .unwrap();
    assert_eq!(due_roots.len(), 21);
    let (notice, settlement) = fixture
        .store
        .select_subscription_notice(&reader, &due_roots, due_at)
        .await
        .unwrap();
    assert_eq!(notice.roots.len(), 20);
    assert_eq!(settlement.roots.len(), 20);
    let included_roots = notice
        .roots
        .iter()
        .map(|root| root.root_id.clone())
        .collect::<Vec<_>>();
    let omitted_roots = due_roots
        .iter()
        .filter(|root| !included_roots.contains(root))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(omitted_roots.len(), 1);

    fixture
        .store
        .settle_subscription_batch(&reader, &settlement, SubscriptionDeliveryOutcome::Accepted)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .store
            .due_subscription_roots(&reader, due_at)
            .await
            .unwrap(),
        omitted_roots
    );
    fixture.finish().await;
}

#[tokio::test]
async fn post_windows_exclude_author_and_topic_roots_keep_independent_timing() {
    let mut fixture = ThreadSubscriptionFixture::create("post-window-author").await;
    fixture
        .post_reply_as(fixture.reader.clone(), "own message", fixture.now)
        .await;
    let thread_record = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert!(thread_record.roots().is_empty());
    fixture.finish().await;

    let mut fixture = ThreadSubscriptionFixture::create_without_participant("topic-windows").await;
    let topic_reader = session("topic-window-reader");
    let topic_scope = SubscriptionScope::topic(fixture.topic_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: topic_reader.clone(),
                scope: topic_scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Poll), None, None, None, None),
            },
            fixture.now,
        )
        .await
        .unwrap();
    let first_arrival = fixture.now + chrono::Duration::seconds(10);
    fixture
        .post_reply_as(human("first-author"), "first root activity", first_arrival)
        .await;
    let second_root = fixture
        .create_root(
            human("root-author"),
            "second root",
            fixture.now + chrono::Duration::seconds(30),
        )
        .await;
    fixture
        .post(
            Placement::Thread {
                root_message_id: second_root.clone(),
            },
            human("second-author"),
            "second root activity",
            fixture.now + chrono::Duration::seconds(40),
        )
        .await;

    let topic_record = record(&mut fixture.store, &topic_reader, &topic_scope).await;
    assert_eq!(topic_record.roots().len(), 2);
    let first_window = topic_record
        .roots()
        .iter()
        .find(|window| window.root_message_id() == &fixture.root_message_id)
        .unwrap();
    let second_window = topic_record
        .roots()
        .iter()
        .find(|window| window.root_message_id() == &second_root)
        .unwrap();
    assert_eq!(first_window.opened_at(), persisted_time(first_arrival));
    assert_eq!(
        second_window.opened_at(),
        persisted_time(fixture.now + chrono::Duration::seconds(30))
    );

    let due_at = fixture.now + chrono::Duration::seconds(130);
    let due_roots = fixture
        .store
        .due_subscription_roots(&topic_reader, due_at)
        .await
        .unwrap();
    assert_eq!(due_roots, vec![fixture.root_message_id.clone()]);
    fixture.finish().await;
}

#[tokio::test]
async fn residual_arrival_opens_at_first_arrival_while_batch_is_in_flight() {
    let mut fixture = ThreadSubscriptionFixture::create("residual-window").await;
    fixture
        .post_reply_as(human("first-author"), "initial activity", fixture.now)
        .await;
    let selection_time = fixture.now + chrono::Duration::seconds(120);
    let due_roots = fixture
        .store
        .due_subscription_roots(&fixture.reader, selection_time)
        .await
        .unwrap();
    let (batch, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due_roots, selection_time)
        .await
        .unwrap();
    assert_eq!(batch.roots.len(), 1);

    let residual_arrival = fixture.now + chrono::Duration::seconds(130);
    fixture
        .post_reply_as(
            human("second-author"),
            "residual activity",
            residual_arrival,
        )
        .await;
    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let residual = record(&mut fixture.store, &fixture.reader, &scope).await;
    assert_eq!(residual.roots().len(), 1);
    let persisted_residual_arrival = persisted_time(residual_arrival);
    assert_eq!(residual.roots()[0].opened_at(), persisted_residual_arrival);
    assert_eq!(
        residual.roots()[0].last_arrival_at(),
        persisted_residual_arrival
    );
    assert_eq!(residual.roots()[0].pending_count(), 1);
    assert!(
        fixture
            .store
            .due_subscription_roots(
                &fixture.reader,
                persisted_residual_arrival + chrono::Duration::seconds(119),
            )
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fixture
            .store
            .due_subscription_roots(
                &fixture.reader,
                persisted_residual_arrival + chrono::Duration::seconds(120),
            )
            .await
            .unwrap(),
        vec![fixture.root_message_id.clone()]
    );
    fixture.finish().await;
}

#[tokio::test]
async fn residual_window_that_reaches_its_cap_is_due_immediately_after_settlement() {
    let mut fixture = ThreadSubscriptionFixture::create("residual-cap").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope,
                policy: policy_patch(None, None, Some(120), Some(600), None),
            },
            fixture.now,
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(human("first-author"), "initial activity", fixture.now)
        .await;
    let select_at = fixture.now + chrono::Duration::seconds(600);
    let due_roots = fixture
        .store
        .due_subscription_roots(&fixture.reader, select_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due_roots, select_at)
        .await
        .unwrap();

    let first_residual = fixture.now + chrono::Duration::seconds(610);
    let last_residual = fixture.now + chrono::Duration::seconds(1_211);
    fixture
        .post_reply_as(human("second-author"), "first residual", first_residual)
        .await;
    fixture
        .post_reply_as(human("third-author"), "late residual", last_residual)
        .await;
    let settle_at = fixture.now + chrono::Duration::seconds(1_212);
    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();
    let residual = record(
        &mut fixture.store,
        &fixture.reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await;
    assert_eq!(
        residual.roots()[0].opened_at(),
        persisted_time(first_residual)
    );
    assert_eq!(
        residual.roots()[0].last_arrival_at(),
        persisted_time(last_residual)
    );
    assert_eq!(
        fixture
            .store
            .due_subscription_roots(&fixture.reader, settle_at)
            .await
            .unwrap(),
        vec![fixture.root_message_id.clone()]
    );
    fixture.finish().await;
}

#[path = "window_recovery_tests.rs"]
mod recovery_tests;
#[path = "settlement_tests.rs"]
mod settlement_tests;
