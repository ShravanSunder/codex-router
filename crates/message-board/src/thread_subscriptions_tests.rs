use crate::{
    ActivitySequence, BatchTiming, EndpointId, HumanId, Identity, MessageId, PendingRootNotice,
    ServiceId, SessionEndpointRef, SessionId, SessionRef, SubscriptionGeneration,
    SubscriptionLifetime, SubscriptionMode, SubscriptionPolicy, SubscriptionPolicyPatch,
    SubscriptionRootRecord, SubscriptionRootRecordProps, SubscriptionState,
    ThreadSubscriptionRecord, ThreadSubscriptionRecordProps, TopicId, WhenIdle,
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
fn subscription_cap_above_sixty_minutes_reports_its_allowed_range() {
    let cap_range = "must be at least quietSeconds and at most 3600 seconds";
    let constructor_error = BatchTiming::new(30 * 60, 3601).unwrap_err();
    assert_eq!(constructor_error.field, "capSeconds");
    assert_eq!(constructor_error.requirement, cap_range);

    let timing_deserialization_error = serde_json::from_value::<BatchTiming>(serde_json::json!({
        "quietSeconds": 1800,
        "capSeconds": 3601
    }))
    .unwrap_err()
    .to_string();
    assert!(timing_deserialization_error.contains(cap_range));

    let policy_deserialization_error =
        serde_json::from_value::<SubscriptionPolicy>(serde_json::json!({
            "mode": "deliver",
            "whenIdle": "hold",
            "timing": { "quietSeconds": 1800, "capSeconds": 3601 },
            "lifetime": 86400
        }))
        .unwrap_err()
        .to_string();
    assert!(policy_deserialization_error.contains(cap_range));

    let policy_patch_error = SubscriptionPolicy::defaults_for(&session_reader())
        .apply_patch(
            &session_reader(),
            &SubscriptionPolicyPatch {
                timing: crate::SubscriptionTimingPatch {
                    quiet_seconds: Some(1800),
                    cap_seconds: Some(3601),
                },
                ..SubscriptionPolicyPatch::default()
            },
        )
        .unwrap_err();
    assert_eq!(policy_patch_error.field, "capSeconds");
    assert_eq!(policy_patch_error.requirement, cap_range);
}

#[test]
fn subscription_lifetime_above_seven_days_reports_its_allowed_range() {
    let lifetime_range = "must be between 600 seconds and 604800 seconds";
    let constructor_error = SubscriptionLifetime::new(604801).unwrap_err();
    assert_eq!(constructor_error.field, "forSeconds");
    assert_eq!(constructor_error.requirement, lifetime_range);

    let lifetime_deserialization_error =
        serde_json::from_value::<SubscriptionLifetime>(serde_json::json!(604801))
            .unwrap_err()
            .to_string();
    assert!(lifetime_deserialization_error.contains(lifetime_range));

    let policy_deserialization_error =
        serde_json::from_value::<SubscriptionPolicy>(serde_json::json!({
            "mode": "deliver",
            "whenIdle": "hold",
            "timing": { "quietSeconds": 120, "capSeconds": 600 },
            "lifetime": 604801
        }))
        .unwrap_err()
        .to_string();
    assert!(policy_deserialization_error.contains(lifetime_range));

    let policy_patch_error = SubscriptionPolicy::defaults_for(&session_reader())
        .apply_patch(
            &session_reader(),
            &SubscriptionPolicyPatch {
                lifetime_seconds: Some(604801),
                ..SubscriptionPolicyPatch::default()
            },
        )
        .unwrap_err();
    assert_eq!(policy_patch_error.field, "forSeconds");
    assert_eq!(policy_patch_error.requirement, lifetime_range);
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

#[test]
fn subscription_value_deserialization_uses_validated_constructors() {
    assert!(
        serde_json::from_value::<BatchTiming>(serde_json::json!({
            "quietSeconds": 1801,
            "capSeconds": 1801
        }))
        .is_err()
    );
    assert!(serde_json::from_value::<SubscriptionLifetime>(serde_json::json!(599)).is_err());
    assert!(serde_json::from_value::<SubscriptionGeneration>(serde_json::json!(0)).is_err());
    assert!(
        serde_json::from_value::<SubscriptionPolicy>(serde_json::json!({
            "mode": "deliver",
            "whenIdle": "hold",
            "timing": { "quietSeconds": 60, "capSeconds": 59 },
            "lifetime": 86400
        }))
        .is_err()
    );

    let now = chrono::Utc::now();
    let root_id = MessageId::generate();
    let root_record = SubscriptionRootRecord::new(SubscriptionRootRecordProps {
        root_message_id: root_id.clone(),
        opened_at: now,
        last_arrival_at: now,
        pending_count: 1,
        held_since: None,
        next_retry_at: None,
        retry_attempts: 0,
    })
    .unwrap();
    let record = ThreadSubscriptionRecord::new(ThreadSubscriptionRecordProps {
        reader: session_reader(),
        scope: crate::SubscriptionScope::thread(root_id),
        policy: SubscriptionPolicy::defaults_for(&session_reader()),
        state: SubscriptionState::Active,
        renewed_at: now,
        expires_at: now + chrono::Duration::hours(24),
        ended_at: None,
        generation: SubscriptionGeneration::new(1).unwrap(),
        last_outcome: None,
        roots: vec![root_record],
    })
    .unwrap();
    let mut encoded_record = serde_json::to_value(record).unwrap();
    encoded_record["reader"] = serde_json::json!({
        "kind": "human",
        "humanId": "person"
    });
    assert!(serde_json::from_value::<ThreadSubscriptionRecord>(encoded_record).is_err());

    let mut encoded_root = serde_json::to_value(
        SubscriptionRootRecord::new(SubscriptionRootRecordProps {
            root_message_id: MessageId::generate(),
            opened_at: now,
            last_arrival_at: now,
            pending_count: 1,
            held_since: None,
            next_retry_at: None,
            retry_attempts: 0,
        })
        .unwrap(),
    )
    .unwrap();
    encoded_root["lastArrivalAt"] = serde_json::json!(now - chrono::Duration::seconds(1));
    assert!(serde_json::from_value::<SubscriptionRootRecord>(encoded_root).is_err());
}
