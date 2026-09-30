use crate::{
    ActivitySequence, BatchTiming, EndpointId, HumanId, Identity, MessageId, PendingRootNotice,
    ServiceId, SessionEndpointRef, SessionId, SessionRef, SubscriptionLifetime, SubscriptionMode,
    SubscriptionPolicy, TopicId, WhenIdle,
};

fn session_reader() -> Identity {
    Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned())
                    .unwrap(),
                endpoint_id: EndpointId::try_from("codex-local".to_owned()).unwrap(),
            },
            session_id: SessionId::try_from("subscription-test".to_owned()).unwrap(),
        },
    }
}

#[test]
fn policy_bounds_and_human_delivery_rule_are_validated_by_constructors() {
    assert!(BatchTiming::new(30 * 60, 30 * 60).is_ok());
    assert!(BatchTiming::new(30 * 60 + 1, 30 * 60 + 1).is_err());
    assert!(BatchTiming::new(60, 59).is_err());
    assert!(SubscriptionLifetime::new(10 * 60).is_ok());
    assert!(SubscriptionLifetime::new(10 * 60 - 1).is_err());
    let human = Identity::Human {
        human_id: HumanId::try_from("person".to_owned()).unwrap(),
    };
    assert!(
        SubscriptionPolicy::new(
            &human,
            SubscriptionMode::Deliver,
            WhenIdle::Hold,
            BatchTiming::defaults(),
            SubscriptionLifetime::defaults(),
        )
        .is_err()
    );
    assert!(
        SubscriptionPolicy::new(
            &session_reader(),
            SubscriptionMode::Deliver,
            WhenIdle::Hold,
            BatchTiming::defaults(),
            SubscriptionLifetime::defaults(),
        )
        .is_ok()
    );
}

#[test]
fn pending_root_notice_rejects_invalid_ranges_counts_authors_and_excerpts() {
    let from_sequence = ActivitySequence::try_from(4_u64).unwrap();
    let through_sequence = ActivitySequence::try_from(7_u64).unwrap();
    let author = session_reader();
    assert!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            from_sequence,
            through_sequence,
            3,
            vec![author.clone()],
            "root title".to_owned(),
        )
        .is_ok()
    );

    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            through_sequence,
            from_sequence,
            3,
            vec![author.clone()],
            "root title".to_owned(),
        )
        .unwrap_err()
        .field,
        "fromSequence"
    );
    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            from_sequence,
            through_sequence,
            0,
            vec![author.clone()],
            "root title".to_owned(),
        )
        .unwrap_err()
        .field,
        "messageCount"
    );
    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            from_sequence,
            through_sequence,
            3,
            vec![author.clone(), author.clone()],
            "root title".to_owned(),
        )
        .unwrap_err()
        .field,
        "authors"
    );
    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            from_sequence,
            through_sequence,
            3,
            vec![author.clone()],
            " title with spaces ".to_owned(),
        )
        .unwrap_err()
        .field,
        "rootTitleExcerpt"
    );
    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            from_sequence,
            through_sequence,
            3,
            vec![author],
            "x".repeat(81),
        )
        .unwrap_err()
        .field,
        "rootTitleExcerpt"
    );
}
