use super::*;

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

async fn get_record(
    store: &mut BoardStore,
    reader: &Identity,
    scope: &SubscriptionScope,
) -> Option<ThreadSubscriptionRecord> {
    store
        .get_thread_subscription_record(reader, scope)
        .await
        .unwrap()
}

#[tokio::test]
async fn join_watch_creates_default_subscription_and_no_watch_does_not() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("join-default").await;
    let watched_reader = fixture.reader.clone();
    let watched = fixture
        .join_reader(
            watched_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;
    assert!(watched.watch_status.watching);
    let watched_record = get_record(
        &mut fixture.store,
        &watched_reader,
        &SubscriptionScope::thread(fixture.root_message_id.clone()),
    )
    .await
    .unwrap();
    assert_eq!(watched_record.state(), SubscriptionState::Active);
    assert_eq!(
        watched_record.policy().clone(),
        SubscriptionPolicy::defaults_for(&watched_reader)
    );

    let opted_out_reader = session("no-watch-reader");
    let opted_out = fixture
        .join_reader(
            opted_out_reader.clone(),
            ParticipantRole::Participant,
            false,
            fixture.now,
        )
        .await;
    assert!(!opted_out.watch_status.watching);
    assert!(
        get_record(
            &mut fixture.store,
            &opted_out_reader,
            &SubscriptionScope::thread(fixture.root_message_id.clone()),
        )
        .await
        .is_none()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn create_with_role_subscribes_the_session_creator() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("create-role").await;
    let creator = session("creator");
    let created = fixture
        .store
        .create_thread(
            ThreadCreateRequest {
                message_id: MessageId::generate(),
                topic_id: fixture.topic_id.clone(),
                actor: creator.clone(),
                acting_for: None,
                text: text("Created with role"),
                references: no_references(),
                role: Some(ParticipantRole::Implementer),
                watch: true,
            },
            fixture.now,
        )
        .await
        .unwrap();
    let scope = SubscriptionScope::thread(created.message.message_id.clone());
    let record = get_record(&mut fixture.store, &creator, &scope)
        .await
        .unwrap();
    assert_eq!(record.state(), SubscriptionState::Active);
    assert_eq!(
        record.policy().clone(),
        SubscriptionPolicy::defaults_for(&creator)
    );
    let watch = fixture
        .store
        .show_thread(ThreadShowRequest {
            root_message_id: created.message.message_id,
            reader: Some(creator),
        })
        .await
        .unwrap()
        .watch_status
        .unwrap();
    assert!(watch.watching);
    fixture.finish().await;
}

#[tokio::test]
async fn rejoin_reactivates_ended_subscription_and_keeps_policy() {
    let mut fixture = ThreadSubscriptionFixture::create("rejoin-policy").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let patched = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: policy_patch(
                    Some(SubscriptionMode::Poll),
                    None,
                    Some(60),
                    Some(600),
                    Some(60 * 60),
                ),
            },
            fixture.now,
        )
        .await
        .unwrap();
    let ended = fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
            },
            fixture.now + chrono::Duration::seconds(10),
        )
        .await
        .unwrap();
    assert_eq!(
        ended.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );

    let rejoined_at = fixture.now + chrono::Duration::seconds(20);
    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            true,
            rejoined_at,
        )
        .await;
    let reactivated = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();
    assert_eq!(reactivated.state(), SubscriptionState::Active);
    assert_eq!(reactivated.policy(), patched.policy());
    assert_eq!(reactivated.renewed_at(), persisted_time(rejoined_at));
    assert!(reactivated.generation().get() > ended.generation().get());
    fixture.finish().await;
}

#[tokio::test]
async fn thread_subscribe_checks_participant_validates_patch_and_activates_watch() {
    let mut fixture =
        ThreadSubscriptionFixture::create_without_participant("subscribe-validation").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let refusal = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap_err();
    assert_eq!(refusal.kind, BoardFailureKind::ParticipantRequired);

    fixture
        .join_reader(
            fixture.reader.clone(),
            ParticipantRole::Participant,
            false,
            fixture.now,
        )
        .await;
    let record = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: policy_patch(
                    Some(SubscriptionMode::Poll),
                    None,
                    Some(60),
                    None,
                    Some(60 * 60),
                ),
            },
            fixture.now,
        )
        .await
        .unwrap();
    assert_eq!(record.policy().mode(), SubscriptionMode::Poll);
    assert_eq!(record.policy().timing().quiet_seconds(), 60);
    assert_eq!(record.policy().timing().cap_seconds(), 600);
    assert!(fixture.watch_status(fixture.reader.clone()).await.watching);

    let invalid = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: policy_patch(None, None, Some(300), Some(120), None),
            },
            fixture.now + chrono::Duration::seconds(30),
        )
        .await
        .unwrap_err();
    assert_eq!(invalid.kind, BoardFailureKind::InvalidField);
    assert_eq!(
        get_record(&mut fixture.store, &fixture.reader, &scope).await,
        Some(record)
    );

    let human_reader = human("non-push-reader");
    let human_scope = SubscriptionScope::topic(fixture.topic_id.clone());
    let invalid_human = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: human_reader.clone(),
                scope: human_scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Deliver), None, None, None, None),
            },
            fixture.now,
        )
        .await
        .unwrap_err();
    assert_eq!(invalid_human.kind, BoardFailureKind::InvalidField);
    assert!(
        get_record(&mut fixture.store, &human_reader, &human_scope)
            .await
            .is_none()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn unsubscribe_keeps_watch_but_unwatch_ends_without_post_reactivation() {
    let mut fixture = ThreadSubscriptionFixture::create("unsubscribe-keeps-watch").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    fixture
        .post_reply("other", "pending before unsubscribe")
        .await;
    let cancelled = fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    assert_eq!(
        cancelled.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    assert!(cancelled.roots().is_empty());
    assert!(fixture.watch_status(fixture.reader.clone()).await.watching);
    fixture
        .post_reply("other", "does not recreate subscription")
        .await;
    let after_post = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();
    assert_eq!(after_post.state(), cancelled.state());
    assert!(after_post.roots().is_empty());

    let unwatch_reader = session("unwatch-reader");
    fixture
        .join_reader(
            unwatch_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;
    fixture
        .post_reply("window-opener", "creates a window")
        .await;
    let opted_out_reader = session("active-then-no-watch");
    fixture
        .join_reader(
            opted_out_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now,
        )
        .await;
    fixture
        .post_reply("after active rejoin", "window before no-watch rejoin")
        .await;
    fixture
        .join_reader(
            opted_out_reader.clone(),
            ParticipantRole::Participant,
            false,
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;
    let opted_out_record = get_record(&mut fixture.store, &opted_out_reader, &scope)
        .await
        .unwrap();
    assert_eq!(
        opted_out_record.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    assert!(opted_out_record.roots().is_empty());

    fixture
        .store
        .unwatch_thread(
            ThreadUnwatchRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: unwatch_reader.clone(),
                acting_for: None,
            },
            fixture.now + chrono::Duration::seconds(3),
        )
        .await
        .unwrap();
    let unwatch_record = get_record(&mut fixture.store, &unwatch_reader, &scope)
        .await
        .unwrap();
    assert_eq!(
        unwatch_record.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    assert!(unwatch_record.roots().is_empty());
    assert!(!fixture.watch_status(unwatch_reader.clone()).await.watching);
    fixture
        .post_reply("later-author", "must not recreate ended subscription")
        .await;
    assert_eq!(
        get_record(&mut fixture.store, &unwatch_reader, &scope)
            .await
            .unwrap()
            .state(),
        unwatch_record.state()
    );

    let topic_reader = session("topic-unwatch-reader");
    let topic_scope = SubscriptionScope::topic(fixture.topic_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: topic_reader.clone(),
                scope: topic_scope.clone(),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now,
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("topic-activity-author"),
            "window before topic unwatch",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    assert_eq!(
        get_record(&mut fixture.store, &topic_reader, &topic_scope)
            .await
            .unwrap()
            .roots()
            .len(),
        1
    );
    fixture
        .store
        .unwatch_topic(
            TopicWatchRequest {
                topic_id: fixture.topic_id.clone(),
                actor: topic_reader.clone(),
                acting_for: None,
            },
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap();
    let topic_record = get_record(&mut fixture.store, &topic_reader, &topic_scope)
        .await
        .unwrap();
    assert_eq!(
        topic_record.state(),
        SubscriptionState::Ended {
            reason: EndReason::Cancelled
        }
    );
    assert!(topic_record.roots().is_empty());
    assert!(
        fixture
            .watch_status_for(fixture.root_message_id.clone(), topic_reader.clone())
            .await
            .watching
    );
    fixture
        .post_reply_as(
            human("after-topic-unwatch"),
            "kept in the watched thread inbox",
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;
    assert!(
        fixture
            .store
            .due_subscription_roots(&topic_reader, fixture.now + chrono::Duration::seconds(200),)
            .await
            .unwrap()
            .is_empty(),
        "the materialized Thread Watch remains, but the ended Topic Subscription does not push"
    );
    fixture.finish().await;
}

#[path = "lifecycle_completion.rs"]
mod completion;
#[path = "lifecycle_renewal.rs"]
mod renewal;
