use super::*;

#[tokio::test]
async fn session_thread_subscriptions_require_join_while_topic_humans_remain_exempt() {
    let mut fixture = Fixture::open("gates").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    let unjoined = session("unjoined");
    let post_failure = fixture
        .store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.clone(),
                },
                actor: unjoined.clone(),
                acting_for: None,
                text: MessageText::try_from("blocked".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(post_failure.kind, BoardFailureKind::ParticipantRequired);
    let resolve_failure = fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: root.clone(),
                actor: unjoined.clone(),
                acting_for: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(resolve_failure.kind, BoardFailureKind::ParticipantRequired);
    let joined_non_orchestrator = session("joined-non-orchestrator");
    fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root.clone(),
                actor: joined_non_orchestrator.clone(),
                role: ParticipantRole::Reviewer,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let resolve_failure = fixture
        .store
        .resolve_thread(
            ThreadResolveRequest {
                root_message_id: root.clone(),
                actor: joined_non_orchestrator,
                acting_for: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(resolve_failure.kind, BoardFailureKind::OrchestratorRequired);
    let subscription_failure = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: unjoined.clone(),
                scope: SubscriptionScope::thread(root.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    ..SubscriptionPolicyPatch::default()
                },
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        subscription_failure.kind,
        BoardFailureKind::ParticipantRequired
    );
    let human_reply = fixture
        .store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.clone(),
                },
                actor: human("human-reader"),
                acting_for: None,
                text: MessageText::try_from("human reply".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(!human_reply.watch_status.watching);
    let human_subscription = fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: human("human-reader"),
                scope: SubscriptionScope::topic(fixture.topic_id.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    ..SubscriptionPolicyPatch::default()
                },
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(human_subscription.policy().mode(), SubscriptionMode::Poll);
    fixture.finish().await;
}

#[tokio::test]
async fn thread_subscriptions_refuse_each_missing_session_participant_without_creating_watches() {
    let mut fixture = Fixture::open("subscription-missing-participants").await;
    let first_root = fixture
        .create(human("owner-one"), None, false)
        .await
        .message
        .message_id;
    let second_root = fixture
        .create(human("owner-two"), None, false)
        .await
        .message
        .message_id;
    let reader = session("unjoined-listener");

    for root_message_id in [first_root.clone(), second_root.clone()] {
        let failure = fixture
            .store
            .subscribe_thread_subscription(
                ThreadSubscriptionSubscribeRequest {
                    reader: reader.clone(),
                    scope: SubscriptionScope::thread(root_message_id.clone()),
                    policy: SubscriptionPolicyPatch {
                        mode: Some(SubscriptionMode::Poll),
                        ..SubscriptionPolicyPatch::default()
                    },
                },
                chrono::Utc::now(),
            )
            .await
            .unwrap_err();
        assert_eq!(failure.kind, BoardFailureKind::ParticipantRequired);
        assert!(matches!(
            &failure.details,
            BoardErrorDetails::ParticipantRefusal { .. }
        ));
        let BoardErrorDetails::ParticipantRefusal { refusal } = failure.details else {
            return;
        };
        assert_eq!(refusal.root_message_id, root_message_id);
        assert_eq!(
            refusal.missing_root_message_ids,
            vec![root_message_id.clone()]
        );
        let thread = fixture
            .store
            .show_thread(ThreadShowRequest {
                root_message_id: root_message_id.clone(),
                reader: Some(reader.clone()),
            })
            .await
            .unwrap();
        assert!(!thread.watch_status.unwrap().watching);
    }
    fixture.finish().await;
}

#[tokio::test]
async fn concurrent_orchestrator_joins_commit_one_holder_and_report_the_winner() {
    let mut fixture = Fixture::open("concurrent-holder").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    fixture.store.close().await.unwrap();
    let mut first_store = BoardStore::open(&fixture.path).await.unwrap();
    let mut second_store = BoardStore::open(&fixture.path).await.unwrap();
    let first = session("first-racer");
    let second = session("second-racer");
    let first_request = ThreadJoinRequest {
        mode: None,
        when_idle: None,
        root_message_id: root.clone(),
        actor: first.clone(),
        role: ParticipantRole::Orchestrator,
        watch: false,
        replace: None,
        note: None,
    };
    let second_request = ThreadJoinRequest {
        mode: None,
        when_idle: None,
        root_message_id: root.clone(),
        actor: second.clone(),
        role: ParticipantRole::Orchestrator,
        watch: false,
        replace: None,
        note: None,
    };
    let (first_result, second_result) = tokio::join!(
        first_store.join_thread(first_request, chrono::Utc::now()),
        second_store.join_thread(second_request, chrono::Utc::now())
    );
    let successes = usize::from(first_result.is_ok()) + usize::from(second_result.is_ok());
    assert_eq!(successes, 1);
    let failure = first_result.err().or_else(|| second_result.err()).unwrap();
    assert_eq!(failure.kind, BoardFailureKind::OrchestratorAlreadyExists);
    assert_eq!(failure.next_action, BoardNextAction::ReplaceOrchestrator);
    let shown = first_store
        .show_thread(ThreadShowRequest {
            root_message_id: root,
            reader: None,
        })
        .await
        .unwrap();
    assert!(matches!(
        shown.thread.orchestrator.unwrap().identity,
        identity if identity == first || identity == second
    ));
    first_store.close().await.unwrap();
    second_store.close().await.unwrap();
    std::fs::remove_file(fixture.path).unwrap();
}

#[tokio::test]
async fn subscription_handoff_advances_last_seen_monotonically_and_lifecycle_is_not_acknowledgeable()
 {
    let mut fixture = Fixture::open("listen-presence").await;
    let created = fixture.create(human("owner"), None, false).await;
    let root = created.message.message_id;
    let reader = session("listener");
    let joined = fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: Some(SubscriptionMode::Poll),
                when_idle: Some(WhenIdle::Hold),
                root_message_id: root.clone(),
                actor: reader.clone(),
                role: ParticipantRole::Reviewer,
                watch: true,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let join_sequence = joined.participant.joined_at_activity;
    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: reader.clone(),
                scope: SubscriptionScope::thread(root.clone()),
                policy: SubscriptionPolicyPatch {
                    mode: Some(SubscriptionMode::Poll),
                    when_idle: Some(WhenIdle::Hold),
                    timing: SubscriptionTimingPatch {
                        quiet_seconds: Some(0),
                        cap_seconds: Some(0),
                    },
                    ..SubscriptionPolicyPatch::default()
                },
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let external_message = fixture
        .store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.clone(),
                },
                actor: human("external"),
                acting_for: None,
                text: MessageText::try_from("external".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let own_message = fixture
        .store
        .post_message(
            MessagePostRequest {
                message_id: MessageId::generate(),
                placement: Placement::Thread {
                    root_message_id: root.clone(),
                },
                actor: reader.clone(),
                acting_for: None,
                text: MessageText::try_from("own newer presence".to_owned()).unwrap(),
                references: MessageReferences::try_from(Vec::new()).unwrap(),
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let selection_time = chrono::Utc::now() + chrono::Duration::seconds(120);
    let due_roots = fixture
        .store
        .due_subscription_roots(&reader, selection_time)
        .await
        .unwrap();
    assert_eq!(due_roots, vec![root.clone()]);
    let (notice, settlement) = fixture
        .store
        .select_subscription_notice(&reader, &due_roots, selection_time, usize::MAX)
        .await
        .unwrap();
    assert_eq!(notice.roots.len(), 1);
    let selected_root_notice = notice.roots.first();
    assert_eq!(
        selected_root_notice.map(|root_notice| root_notice.through_sequence),
        Some(external_message.message.activity_sequence),
        "the subscription notice should end at the external reply"
    );
    assert!(
        selected_root_notice.is_some_and(|root_notice| root_notice.from_sequence > join_sequence),
        "the subscription notice should begin after the session joined"
    );
    assert!(!serde_json::to_string(&notice).unwrap().contains("external"));
    fixture
        .store
        .settle_subscription_batch(&reader, &settlement, SubscriptionDeliveryOutcome::Accepted)
        .await
        .unwrap();
    let participant = fixture
        .store
        .list_thread_participants(ThreadParticipantListRequest {
            root_message_id: root.clone(),
            page: page(100),
        })
        .await
        .unwrap()
        .page
        .records
        .into_iter()
        .find(|participant| participant.identity == reader)
        .unwrap();
    assert_eq!(
        participant.last_seen_activity, own_message.message.activity_sequence,
        "an older subscription settlement cannot reduce last-seen"
    );
    let acknowledgement = fixture
        .store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: human("owner"),
            acting_for: None,
            scope: ReadScope::Thread {
                root_message_id: root.clone(),
            },
            through_activity_sequence: join_sequence,
        })
        .await
        .unwrap_err();
    assert_eq!(
        acknowledgement.kind,
        BoardFailureKind::InvalidAcknowledgement
    );
    fixture
        .store
        .watch_thread(ThreadWatchRequest {
            root_message_id: root.clone(),
            actor: human("observer"),
            acting_for: None,
        })
        .await
        .unwrap();
    fixture
        .store
        .join_thread(
            ThreadJoinRequest {
                mode: None,
                when_idle: None,
                root_message_id: root,
                actor: session("later-participant"),
                role: ParticipantRole::Advisor,
                watch: false,
                replace: None,
                note: None,
            },
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    fixture
        .store
        .fetch_inbox(InboxFetchRequest {
            scope: InboxScope::Project {
                project_id: fixture.project_id.clone(),
            },
            read_mode: InboxReadMode::Latest,
            reader: human("observer"),
            page: page(100),
        })
        .await
        .expect("Participant lifecycle Activity is excluded from inbox decoding");
    fixture.finish().await;
}
