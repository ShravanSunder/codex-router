use super::*;
use sqlx::{Connection, SqliteConnection};

async fn raw_connection(path: &std::path::Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap()
}

async fn in_flight_through(path: &std::path::Path, root_id: &MessageId) -> Option<i64> {
    let mut connection = raw_connection(path).await;
    let through: Option<Option<i64>> =
        sqlx::query_scalar("SELECT in_flight_through FROM subscription_windows WHERE root_id=?")
            .bind(root_id.as_str())
            .fetch_optional(&mut connection)
            .await
            .unwrap();
    connection.close().await.unwrap();
    through.flatten()
}

fn project_inbox_request(reader: Identity, project_id: ProjectId) -> InboxFetchRequest {
    InboxFetchRequest {
        scope: InboxScope::Project { project_id },
        reader,
        read_mode: InboxReadMode::Unread,
        page: PageRequest {
            limit: 100_u32.try_into().unwrap(),
            cursor: None,
        },
    }
}

#[tokio::test]
async fn p2_topic_root_notice_settlement_keeps_inbox_fetch_and_ack_valid() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("review-p2").await;
    fixture
        .store
        .fetch_inbox(project_inbox_request(
            fixture.reader.clone(),
            fixture.project_id.clone(),
        ))
        .await
        .unwrap();
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    let new_root = fixture
        .create_root(
            human("review-p2-author"),
            "new root",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let due_at = fixture.now + chrono::Duration::seconds(200);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    assert!(due.contains(&new_root));
    let (batch, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, due_at, usize::MAX)
        .await
        .unwrap();
    assert!(batch.roots.iter().any(|root| root.root_id == new_root));
    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();

    let inbox = fixture
        .store
        .fetch_inbox(project_inbox_request(
            fixture.reader.clone(),
            fixture.project_id.clone(),
        ))
        .await
        .unwrap();
    assert!(inbox.page.records.iter().any(|record| matches!(
        record,
        InboxActivity::MessageCreated { message, .. } if message.message_id == new_root
    )));
    let (acknowledgement_scope, through_activity_sequence) = inbox
        .page
        .records
        .iter()
        .find_map(|record| match record {
            InboxActivity::MessageCreated {
                acknowledgement_scope,
                message,
                ..
            } if message.message_id == new_root => {
                Some((acknowledgement_scope.clone(), message.activity_sequence))
            }
            _ => None,
        })
        .unwrap();
    fixture
        .store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: fixture.reader.clone(),
            acting_for: None,
            scope: acknowledgement_scope,
            through_activity_sequence,
        })
        .await
        .unwrap();
    fixture.finish().await;
}

#[tokio::test]
async fn p3_late_settlement_after_rejoin_cannot_cross_the_new_watch_boundary() {
    let mut fixture = ThreadSubscriptionFixture::create("review-p3").await;
    let first = fixture.post_reply("review-p3-author", "before leave").await;
    let first_due_at = fixture.now + chrono::Duration::seconds(121);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, first_due_at)
        .await
        .unwrap();
    let (_, old_settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, first_due_at, usize::MAX)
        .await
        .unwrap();

    fixture
        .store
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: fixture.reader.clone(),
                to: None,
                resolve: false,
            },
            first_due_at + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("review-p3-author"),
            "while away",
            first_due_at + chrono::Duration::seconds(2),
        )
        .await;
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            true,
            first_due_at + chrono::Duration::seconds(3),
        )
        .await;
    let after_rejoin = fixture
        .post_reply_as(
            human("review-p3-author"),
            "after rejoin",
            first_due_at + chrono::Duration::seconds(4),
        )
        .await;

    fixture
        .store
        .settle_subscription_batch(
            &fixture.reader,
            &old_settlement,
            SubscriptionDeliveryOutcome::Accepted,
        )
        .await
        .unwrap();
    let due_at = first_due_at + chrono::Duration::seconds(400);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    assert_eq!(due, vec![fixture.root_message_id.clone()]);
    let (notice, _) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, due_at, usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        notice.roots[0].from_sequence,
        after_rejoin.message.activity_sequence
    );
    assert_eq!(
        notice.roots[0].through_sequence,
        after_rejoin.message.activity_sequence
    );
    assert!(after_rejoin.message.activity_sequence > first.message.activity_sequence);
    fixture
        .store
        .fetch_inbox(project_inbox_request(
            fixture.reader.clone(),
            fixture.project_id.clone(),
        ))
        .await
        .unwrap();
    fixture
        .post_reply_as(
            fixture.reader.clone(),
            "reader post after rejoin",
            first_due_at + chrono::Duration::seconds(401),
        )
        .await;
    fixture.finish().await;
}

#[tokio::test]
async fn p4_policy_patch_during_flight_clears_the_matching_window() {
    let mut fixture = ThreadSubscriptionFixture::create("review-p4").await;
    fixture.post_reply("review-p4-author", "selected").await;
    let selected_at = fixture.now + chrono::Duration::seconds(121);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, selected_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, selected_at, usize::MAX)
        .await
        .unwrap();
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root_message_id.clone()),
                policy: SubscriptionPolicyPatch {
                    timing: SubscriptionTimingPatch {
                        quiet_seconds: Some(60),
                        cap_seconds: None,
                    },
                    ..SubscriptionPolicyPatch::default()
                },
            },
            selected_at + chrono::Duration::seconds(1),
        )
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
    assert_eq!(
        in_flight_through(&fixture.path, &fixture.root_message_id).await,
        None
    );

    fixture
        .post_reply_as(
            human("review-p4-author"),
            "new arrival",
            selected_at + chrono::Duration::seconds(2),
        )
        .await;
    let later = selected_at + chrono::Duration::seconds(2_000);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, later)
        .await
        .unwrap();
    let (next, _) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, later, usize::MAX)
        .await
        .unwrap();
    assert_eq!(next.roots.len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn p5_resolve_during_flight_retry_clears_in_flight_and_allows_selection() {
    let mut fixture = ThreadSubscriptionFixture::create("review-p5").await;
    fixture.post_reply("review-p5-author", "selected").await;
    let selected_at = fixture.now + chrono::Duration::seconds(121);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, selected_at)
        .await
        .unwrap();
    let (_, settlement) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, selected_at, usize::MAX)
        .await
        .unwrap();
    fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: human("owner"),
                acting_for: None,
            },
            selected_at + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    fixture
        .store
        .mark_subscription_batch_retry(
            &fixture.reader,
            &settlement,
            selected_at + chrono::Duration::seconds(2),
            selected_at + chrono::Duration::seconds(32),
            SubscriptionDeliveryOutcome::Rejected {
                evidence: serde_json::json!({}),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        in_flight_through(&fixture.path, &fixture.root_message_id).await,
        None
    );
    let later = selected_at + chrono::Duration::seconds(2_000);
    let due = fixture
        .store
        .due_subscription_roots(&fixture.reader, later)
        .await
        .unwrap();
    let (retry, _) = fixture
        .store
        .select_subscription_notice(&fixture.reader, &due, later, usize::MAX)
        .await
        .unwrap();
    assert_eq!(retry.roots.len(), 1);
    fixture.finish().await;
}

#[tokio::test]
async fn p6_topic_coverage_does_not_reactivate_an_explicitly_unwatched_thread() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("review-p6").await;
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    fixture
        .store
        .unwatch_thread(
            ThreadUnwatchRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: fixture.reader.clone(),
                acting_for: None,
            },
            fixture.now + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    let cancelled = fixture
        .store
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root_message_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cancelled.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    fixture
        .post_reply_as(
            human("review-p6-author"),
            "must not reactivate the watch",
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;
    let watch = fixture.watch_status(fixture.reader.clone()).await;
    assert!(!watch.watching);
    assert!(
        fixture
            .store
            .due_subscription_roots(
                &fixture.reader,
                fixture.now + chrono::Duration::seconds(200)
            )
            .await
            .unwrap()
            .is_empty()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn p6_join_without_watch_writes_thread_override_over_topic_coverage() {
    let mut fixture =
        ThreadSubscriptionFixture::create_without_participant("review-p6-no-watch").await;
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            false,
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let cancelled = fixture
        .store
        .get_thread_subscription_record(
            &fixture.reader,
            &SubscriptionScope::thread(fixture.root_message_id.clone()),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cancelled.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    fixture.finish().await;
}

#[tokio::test]
async fn p7_drop_policy_processes_a_draining_subscription() {
    let mut fixture = ThreadSubscriptionFixture::create("review-p7").await;
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::thread(fixture.root_message_id.clone()),
                policy: SubscriptionPolicyPatch {
                    when_idle: Some(WhenIdle::Drop),
                    ..SubscriptionPolicyPatch::default()
                },
            },
            fixture.now,
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("review-p7-author"),
            "pending before resolution",
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
    fixture
        .store
        .drop_subscription_roots(
            &fixture.reader,
            std::slice::from_ref(&fixture.root_message_id),
            fixture.now + chrono::Duration::seconds(3),
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
    assert_eq!(record.state(), SubscriptionState::Draining);
    assert!(record.roots().is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn p8_rejoin_after_unsubscribe_starts_after_activity_while_unsubscribed() {
    let mut fixture = ThreadSubscriptionFixture::create("review-p8").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
            },
            fixture.now + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    for offset in 0..3 {
        fixture
            .post_reply_as(
                human("review-p8-author"),
                &format!("while unsubscribed {offset}"),
                fixture.now + chrono::Duration::seconds(2 + offset),
            )
            .await;
    }
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now + chrono::Duration::seconds(10),
        )
        .await;
    fixture
        .store
        .restore_active_and_draining(fixture.now + chrono::Duration::seconds(11))
        .await
        .unwrap();
    let due = fixture
        .store
        .due_subscription_roots(
            &fixture.reader,
            fixture.now + chrono::Duration::seconds(1_000),
        )
        .await
        .unwrap();
    assert!(due.is_empty());
    let record = fixture
        .store
        .get_thread_subscription_record(&fixture.reader, &scope)
        .await
        .unwrap()
        .unwrap();
    assert!(record.roots().is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn p8_first_join_by_existing_watcher_starts_after_prior_activity() {
    let mut fixture =
        ThreadSubscriptionFixture::create_without_participant("review-p8-watching").await;
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: fixture.root_message_id.clone(),
            actor: fixture.reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("review-p8-author"),
            "before participant join",
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
    let due = fixture
        .store
        .due_subscription_roots(
            &fixture.reader,
            fixture.now + chrono::Duration::seconds(200),
        )
        .await
        .unwrap();
    assert!(due.is_empty());
    fixture.finish().await;
}

#[tokio::test]
async fn q1_rejoin_while_active_keeps_pending_window() {
    let mut fixture = ThreadSubscriptionFixture::create("review-q1").await;
    fixture
        .post_reply_as(
            human("review-q1-author"),
            "pending before active rejoin",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let due_at = fixture.now + chrono::Duration::seconds(200);
    let due_before_rejoin = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    assert_eq!(due_before_rejoin, vec![fixture.root_message_id.clone()]);

    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now + chrono::Duration::seconds(30),
        )
        .await;

    let due_after_rejoin = fixture
        .store
        .due_subscription_roots(&fixture.reader, due_at)
        .await
        .unwrap();
    assert_eq!(
        due_after_rejoin,
        vec![fixture.root_message_id.clone()],
        "rejoining an already-active subscription must preserve pending activity"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn q2_topic_subscribe_reactivates_previously_unwatched_root_without_thread_row() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("review-q2").await;
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: fixture.root_message_id.clone(),
            actor: fixture.reader.clone(),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
        .store
        .unwatch_thread(
            ThreadUnwatchRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: fixture.reader.clone(),
                acting_for: None,
            },
            fixture.now,
        )
        .await
        .unwrap();
    assert!(
        fixture
            .store
            .get_thread_subscription_record(
                &fixture.reader,
                &SubscriptionScope::thread(fixture.root_message_id.clone()),
            )
            .await
            .unwrap()
            .is_none()
    );

    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("review-q2-author"),
            "reply after topic subscribe",
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;

    let watch = fixture.watch_status(fixture.reader.clone()).await;
    let due = fixture
        .store
        .due_subscription_roots(
            &fixture.reader,
            fixture.now + chrono::Duration::seconds(200),
        )
        .await
        .unwrap();
    assert!(
        watch.watching && due == vec![fixture.root_message_id.clone()],
        "explicit Topic subscribe must reactivate a previously inactive root Watch; watching={}, due={due:?}",
        watch.watching
    );
    fixture.finish().await;
}
