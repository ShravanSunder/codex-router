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
fn pending_root_notice_contains_only_neutral_activity_locators() {
    let from_sequence = ActivitySequence::try_from(4_u64).unwrap();
    let through_sequence = ActivitySequence::try_from(7_u64).unwrap();
    let notice = PendingRootNotice::new(
        MessageId::generate(),
        TopicId::generate(),
        from_sequence,
        through_sequence,
        3,
    )
    .unwrap();
    let encoded_notice = serde_json::to_value(notice).unwrap();
    let notice_fields = encoded_notice.as_object().unwrap();
    assert_eq!(notice_fields.len(), 5);
    assert!(notice_fields.contains_key("rootId"));
    assert!(notice_fields.contains_key("topicId"));
    assert!(notice_fields.contains_key("fromSequence"));
    assert!(notice_fields.contains_key("throughSequence"));
    assert!(notice_fields.contains_key("messageCount"));
    assert!(!notice_fields.contains_key("authors"));
    assert!(!notice_fields.contains_key("rootTitleExcerpt"));

    assert_eq!(
        PendingRootNotice::new(
            MessageId::generate(),
            TopicId::generate(),
            through_sequence,
            from_sequence,
            3,
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
        )
        .unwrap_err()
        .field,
        "messageCount"
    );
}
