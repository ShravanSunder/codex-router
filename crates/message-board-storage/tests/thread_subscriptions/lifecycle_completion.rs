use super::*;

#[tokio::test]
async fn leave_replacement_and_expiry_end_rows_and_delete_windows() {
    let mut left_fixture = ThreadSubscriptionFixture::create("left-end-reason").await;
    left_fixture
        .post_reply("other", "pending before leave")
        .await;
    left_fixture
        .store
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: left_fixture.root_message_id.clone(),
                actor: left_fixture.reader.clone(),
                to: None,
                resolve: false,
            },
            left_fixture.now,
        )
        .await
        .unwrap();
    let left_scope = SubscriptionScope::thread(left_fixture.root_message_id.clone());
    let left_record = get_record(&mut left_fixture.store, &left_fixture.reader, &left_scope)
        .await
        .unwrap();
    assert_eq!(
        left_record.state,
        SubscriptionState::Ended {
            reason: EndReason::Left
        }
    );
    assert!(left_record.roots.is_empty());
    left_fixture.finish().await;

    let mut replaced_fixture =
        ThreadSubscriptionFixture::create_without_participant("replaced-end-reason").await;
    replaced_fixture
        .join_reader(
            replaced_fixture.reader.clone(),
            ParticipantRole::Implementer,
            true,
            replaced_fixture.now,
        )
        .await;
    replaced_fixture
        .post_reply("replacement-test-activity", "window before replacement")
        .await;
    assert_eq!(
        get_record(
            &mut replaced_fixture.store,
            &replaced_fixture.reader,
            &SubscriptionScope::thread(replaced_fixture.root_message_id.clone()),
        )
        .await
        .unwrap()
        .roots
        .len(),
        1
    );
    let replacement_reader = session("replacement-reader");
    replaced_fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                root_message_id: replaced_fixture.root_message_id.clone(),
                actor: replacement_reader,
                role: ParticipantRole::Implementer,
                watch: true,
                replace: Some(replaced_fixture.reader.clone()),
                note: None,
            },
            replaced_fixture.now + chrono::Duration::seconds(1),
        )
        .await
        .unwrap();
    let replaced_scope = SubscriptionScope::thread(replaced_fixture.root_message_id.clone());
    let replaced_record = get_record(
        &mut replaced_fixture.store,
        &replaced_fixture.reader,
        &replaced_scope,
    )
    .await
    .unwrap();
    assert_eq!(
        replaced_record.state,
        SubscriptionState::Ended {
            reason: EndReason::Replaced
        }
    );
    assert!(replaced_record.roots.is_empty());
    replaced_fixture.finish().await;

    let mut expired_fixture = ThreadSubscriptionFixture::create("expired-end-reason").await;
    expired_fixture
        .post_reply("other", "pending before expiry")
        .await;
    let expiry_time = expired_fixture.now + chrono::Duration::hours(25);
    assert_eq!(
        expired_fixture
            .store
            .end_expired_subscriptions(expiry_time)
            .await
            .unwrap(),
        1
    );
    let expired_scope = SubscriptionScope::thread(expired_fixture.root_message_id.clone());
    let expired_record = get_record(
        &mut expired_fixture.store,
        &expired_fixture.reader,
        &expired_scope,
    )
    .await
    .unwrap();
    assert_eq!(
        expired_record.state,
        SubscriptionState::Ended {
            reason: EndReason::Expired
        }
    );
    assert!(expired_record.roots.is_empty());
    expired_fixture.finish().await;
}

#[tokio::test]
async fn resolve_drains_pending_or_ends_mode_off_subscription() {
    let mut draining_fixture =
        ThreadSubscriptionFixture::create_without_participant("resolve-draining").await;
    draining_fixture
        .join_reader(
            draining_fixture.reader.clone(),
            ParticipantRole::Orchestrator,
            true,
            draining_fixture.now,
        )
        .await;
    draining_fixture
        .post_reply("other", "pending when resolved")
        .await;
    draining_fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: draining_fixture.root_message_id.clone(),
                actor: draining_fixture.reader.clone(),
                acting_for: None,
            },
            draining_fixture.now,
        )
        .await
        .unwrap();
    let draining_scope = SubscriptionScope::thread(draining_fixture.root_message_id.clone());
    let draining_record = get_record(
        &mut draining_fixture.store,
        &draining_fixture.reader,
        &draining_scope,
    )
    .await
    .unwrap();
    assert_eq!(draining_record.state, SubscriptionState::Draining);
    assert_eq!(draining_record.roots.len(), 1);
    assert!(
        draining_fixture
            .watch_status(draining_fixture.reader.clone())
            .await
            .watching
    );
    assert_eq!(
        draining_fixture
            .store
            .due_subscription_roots(
                &draining_fixture.reader,
                draining_fixture.now + chrono::Duration::seconds(120),
            )
            .await
            .unwrap(),
        vec![draining_fixture.root_message_id.clone()]
    );
    draining_fixture.finish().await;

    let mut off_fixture =
        ThreadSubscriptionFixture::create_without_participant("resolve-off").await;
    off_fixture
        .join_reader(
            off_fixture.reader.clone(),
            ParticipantRole::Orchestrator,
            true,
            off_fixture.now,
        )
        .await;
    let off_scope = SubscriptionScope::thread(off_fixture.root_message_id.clone());
    off_fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: off_fixture.reader.clone(),
                scope: off_scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Off), None, None, None, None),
            },
            off_fixture.now,
        )
        .await
        .unwrap();
    off_fixture.post_reply("other", "inbox only").await;
    off_fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: off_fixture.root_message_id.clone(),
                actor: off_fixture.reader.clone(),
                acting_for: None,
            },
            off_fixture.now,
        )
        .await
        .unwrap();
    let off_record = get_record(&mut off_fixture.store, &off_fixture.reader, &off_scope)
        .await
        .unwrap();
    assert_eq!(
        off_record.state,
        SubscriptionState::Ended {
            reason: EndReason::Resolved
        }
    );
    assert!(off_record.roots.is_empty());
    off_fixture.finish().await;
}

#[tokio::test]
async fn leave_resolve_ends_resolver_as_left_and_drains_other_readers() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("leave-resolve").await;
    let resolver = session("resolving-orchestrator");
    let other_reader = session("other-reader");
    fixture
        .join_reader(
            resolver.clone(),
            ParticipantRole::Orchestrator,
            true,
            fixture.now,
        )
        .await;
    fixture
        .join_reader(
            other_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    fixture
        .post_reply_as(
            human("pending-author"),
            "pending before resolver leaves",
            fixture.now + chrono::Duration::seconds(2),
        )
        .await;

    fixture
        .store
        .leave_thread(
            ThreadLeaveRequest {
                root_message_id: fixture.root_message_id.clone(),
                actor: resolver.clone(),
                to: None,
                resolve: true,
            },
            fixture.now + chrono::Duration::seconds(3),
        )
        .await
        .unwrap();

    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let resolver_record = get_record(&mut fixture.store, &resolver, &scope)
        .await
        .unwrap();
    assert_eq!(
        resolver_record.state,
        SubscriptionState::Ended {
            reason: EndReason::Left
        }
    );
    assert!(resolver_record.roots.is_empty());
    assert!(!fixture.watch_status(resolver.clone()).await.watching);

    let other_record = get_record(&mut fixture.store, &other_reader, &scope)
        .await
        .unwrap();
    assert_eq!(other_record.state, SubscriptionState::Draining);
    assert_eq!(other_record.roots.len(), 1);
    assert_eq!(other_record.roots[0].pending_count, 1);
    assert!(fixture.watch_status(other_reader.clone()).await.watching);
    assert_eq!(
        fixture
            .store
            .due_subscription_roots(&other_reader, fixture.now + chrono::Duration::seconds(130),)
            .await
            .unwrap(),
        vec![fixture.root_message_id.clone()]
    );
    fixture.finish().await;
}

#[tokio::test]
async fn topic_subscription_keeps_off_activity_unread_and_thread_rows_take_precedence() {
    let mut fixture = ThreadSubscriptionFixture::create_without_participant("topic-watches").await;
    let topic_reader = session("topic-off-reader");
    let topic_scope = SubscriptionScope::topic(fixture.topic_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: topic_reader.clone(),
                scope: topic_scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Off), None, None, None, None),
            },
            fixture.now,
        )
        .await
        .unwrap();
    assert!(
        fixture
            .watch_status_for(fixture.root_message_id.clone(), topic_reader.clone())
            .await
            .watching
    );
    let existing_reply = fixture
        .post_reply_as(
            human("other-author"),
            "off mode remains unread",
            fixture.now + chrono::Duration::seconds(1),
        )
        .await;
    let new_root = fixture
        .store
        .create_thread(
            ThreadCreateRequest {
                message_id: MessageId::generate(),
                topic_id: fixture.topic_id.clone(),
                actor: session("new-root-creator"),
                acting_for: None,
                text: text("root created after topic subscribe"),
                references: no_references(),
                role: Some(ParticipantRole::Participant),
                watch: false,
            },
            fixture.now + chrono::Duration::seconds(2),
        )
        .await
        .unwrap()
        .message
        .message_id;
    let new_reply = fixture
        .post(
            Placement::Thread {
                root_message_id: new_root.clone(),
            },
            human("new-root-author"),
            "new root reply",
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;
    assert!(
        fixture
            .watch_status_for(new_root.clone(), topic_reader.clone())
            .await
            .watching
    );
    let topic_record = get_record(&mut fixture.store, &topic_reader, &topic_scope)
        .await
        .unwrap();
    assert!(topic_record.roots.is_empty());
    let unread_messages = fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            reader: topic_reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest {
                limit: PageLimit::try_from(100).unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap()
        .page
        .records
        .into_iter()
        .filter_map(|activity| match activity {
            InboxActivity::MessageCreated { message, .. } => Some(message.message_id),
            InboxActivity::ThreadStateChanged { .. } => None,
        })
        .collect::<Vec<_>>();
    assert!(unread_messages.contains(&existing_reply.message.message_id));
    assert!(unread_messages.contains(&new_reply.message.message_id));
    let listen_context = fixture
        .store
        .prepare_thread_listen(&ThreadListenRequest {
            reader: topic_reader.clone(),
            selection: ThreadListenSelection::Watched,
            mode: ThreadListenMode::Once {
                max_wait_seconds: 1,
            },
            from_activity_sequence: None,
            acknowledge: false,
            delivery: ThreadListenDelivery::Stdout,
        })
        .await
        .unwrap();
    let selectable = fixture
        .store
        .select_pending_thread_listen_batch_set(ListenId::generate(), &listen_context, 32_000)
        .await
        .unwrap();
    let selectable_ids = selectable
        .batches
        .iter()
        .flat_map(|batch| {
            batch
                .messages
                .iter()
                .map(|message| message.message_id.clone())
        })
        .collect::<Vec<_>>();
    assert!(selectable_ids.contains(&existing_reply.message.message_id));
    assert!(selectable_ids.contains(&new_reply.message.message_id));

    let thread_reader = session("thread-precedence-reader");
    fixture
        .join_reader(
            thread_reader.clone(),
            ParticipantRole::Participant,
            true,
            fixture.now + chrono::Duration::seconds(3),
        )
        .await;
    let thread_scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: thread_reader.clone(),
                scope: thread_scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Off), None, None, None, None),
            },
            fixture.now + chrono::Duration::seconds(4),
        )
        .await
        .unwrap();
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: thread_reader.clone(),
                scope: topic_scope.clone(),
                policy: SubscriptionPolicyPatch::default(),
            },
            fixture.now + chrono::Duration::seconds(5),
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("other-author"),
            "active thread row shadows topic",
            fixture.now + chrono::Duration::seconds(6),
        )
        .await;
    assert!(
        get_record(&mut fixture.store, &thread_reader, &thread_scope)
            .await
            .unwrap()
            .roots
            .is_empty()
    );
    fixture
        .store
        .unsubscribe_thread_subscription(
            ThreadSubscriptionUnsubscribeRequest {
                reader: thread_reader.clone(),
                scope: thread_scope.clone(),
            },
            fixture.now + chrono::Duration::seconds(7),
        )
        .await
        .unwrap();
    fixture
        .post_reply_as(
            human("other-author"),
            "ended thread row still shadows topic",
            fixture.now + chrono::Duration::seconds(8),
        )
        .await;
    assert!(
        get_record(&mut fixture.store, &thread_reader, &topic_scope)
            .await
            .unwrap()
            .roots
            .is_empty()
    );
    fixture.finish().await;
}
