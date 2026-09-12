use project_board::{
    ActingForIdentity, ActivitySequence, BoardId, HumanId, Identity, MessageId, MessageListScope,
    MessagePostRequest, MessageReferences, MessageSelection, MessageText, NormalizedOrigin,
    Placement, ProjectId, ReferenceTarget, ResourceName, ServiceId, SessionEndpointRef, SessionId,
    SessionRef, TopicId, normalize_git_origin_url,
};
use serde_json::json;

fn id_string(sequence_suffix: u32) -> String {
    format!("01890f2e-7b4c-7cc0-98c4-{sequence_suffix:012x}")
}

#[test]
fn resource_ids_accept_only_canonical_uuid_v7() {
    let project_id = ProjectId::try_from(id_string(1)).expect("fixture is UUIDv7");
    assert_eq!(project_id.as_str(), id_string(1));

    assert!(ProjectId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned()).is_err());
    assert!(ProjectId::try_from(id_string(2).to_uppercase()).is_err());
}

#[test]
fn metadata_names_are_trimmed_and_byte_bounded() {
    let name = ResourceName::try_from("  Release coordination  ".to_owned())
        .expect("trimmed name is valid");
    assert_eq!(name.as_str(), "Release coordination");
    assert!(ResourceName::try_from("   ".to_owned()).is_err());
    assert!(ResourceName::try_from("é".repeat(129)).is_err());
}

#[test]
fn session_identity_retains_the_complete_endpoint() {
    let identity = Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from("550e8400-e29b-41d4-a716-446655440000".to_owned())
                    .expect("canonical service ID"),
                endpoint_id: "local-codex".to_owned().try_into().expect("endpoint ID"),
            },
            session_id: SessionId::try_from("thread-42".to_owned()).expect("session ID"),
        },
    };

    assert_eq!(
        serde_json::to_value(&identity).expect("serialize identity"),
        json!({
            "kind": "session",
            "session": {
                "endpoint": {
                    "serviceId": "550e8400-e29b-41d4-a716-446655440000",
                    "endpointId": "local-codex"
                },
                "sessionId": "thread-42"
            }
        })
    );
    assert!(
        serde_json::from_value::<Identity>(json!({
            "kind": "human", "humanId": "owner", "unexpected": true
        }))
        .is_err()
    );
    assert!(HumanId::try_from("owner\0shadow".to_owned()).is_err());
}

#[test]
fn acting_for_contract_can_only_represent_a_human() {
    let acting_for = ActingForIdentity::Human {
        human_id: HumanId::try_from("owner".to_owned()).expect("human ID"),
    };
    assert_eq!(
        serde_json::to_value(acting_for).expect("serialize acting-for"),
        json!({
            "kind": "human", "humanId": "owner"
        })
    );
    assert!(serde_json::from_value::<ActingForIdentity>(json!({
        "kind": "session",
        "session": {"endpoint": {"serviceId": "550e8400-e29b-41d4-a716-446655440000", "endpointId": "local"}, "sessionId": "x"}
    })).is_err());
}

#[test]
fn message_contract_uses_closed_camel_case_variants() {
    let request = MessagePostRequest {
        message_id: MessageId::try_from(id_string(4)).expect("message ID"),
        placement: Placement::Thread {
            root_message_id: MessageId::try_from(id_string(3)).expect("root ID"),
        },
        actor: Identity::Human {
            human_id: HumanId::try_from("reviewer".to_owned()).expect("human ID"),
        },
        acting_for: None,
        text: MessageText::try_from("Please inspect this edge.".to_owned()).expect("message text"),
        references: MessageReferences::try_from(vec![ReferenceTarget::Message {
            message_id: MessageId::try_from(id_string(2)).expect("target ID"),
        }])
        .expect("references"),
    };
    let encoded = serde_json::to_value(request).expect("serialize request");
    assert_eq!(encoded["placement"]["kind"], "thread");
    assert_eq!(encoded["placement"]["rootMessageId"], id_string(3));
    assert_eq!(encoded["references"][0]["kind"], "message");
}

#[test]
fn duplicate_and_excess_references_are_rejected() {
    let target = ReferenceTarget::Thread {
        root_message_id: MessageId::try_from(id_string(1)).expect("root ID"),
    };
    assert!(MessageReferences::try_from(vec![target.clone(), target]).is_err());

    let references: Vec<ReferenceTarget> = (1..=65)
        .map(|suffix| ReferenceTarget::Message {
            message_id: MessageId::try_from(id_string(suffix)).expect("message ID"),
        })
        .collect();
    assert!(MessageReferences::try_from(references).is_err());
}

#[test]
fn history_variants_preserve_scope_and_validate_range_order() {
    let scope = MessageListScope::Project {
        project_id: ProjectId::try_from(id_string(1)).expect("project ID"),
    };
    let selection = MessageSelection::AfterPosition {
        after_activity_sequence: ActivitySequence::try_from(40).expect("activity sequence"),
    };
    assert_eq!(
        serde_json::to_value(scope).expect("serialize scope")["kind"],
        "project"
    );
    assert_eq!(
        serde_json::to_value(selection).expect("serialize selection"),
        json!({
            "kind": "afterPosition", "afterActivitySequence": 40
        })
    );
    assert!(
        MessageSelection::Range {
            from_activity_sequence: ActivitySequence::try_from(41).expect("activity sequence"),
            to_activity_sequence: ActivitySequence::try_from(40).expect("activity sequence"),
        }
        .validate()
        .is_err()
    );
    assert!(
        serde_json::from_value::<MessageSelection>(json!({
            "kind": "range",
            "fromActivitySequence": 41,
            "toActivitySequence": 40
        }))
        .is_err()
    );
}

#[test]
fn activity_positions_fit_the_sqlite_signed_integer_domain() {
    assert_eq!(ActivitySequence::ZERO.get(), 0);
    assert_eq!(
        ActivitySequence::try_from(i64::MAX as u64)
            .expect("maximum SQLite integer")
            .get(),
        i64::MAX as u64
    );
    assert!(ActivitySequence::try_from(i64::MAX as u64 + 1).is_err());
    assert!(serde_json::from_value::<ActivitySequence>(json!(i64::MAX as u64 + 1)).is_err());
}

#[test]
fn repository_origins_strip_credentials_and_preserve_path_case() {
    assert_eq!(
        normalize_git_origin_url("https://token@GitHub.COM/ShravanSunder/MyRepo.git?x=1#fragment"),
        Some("github.com/ShravanSunder/MyRepo".to_owned())
    );
    assert_eq!(
        normalize_git_origin_url("git@github.com:ShravanSunder/MyRepo.git"),
        Some("github.com/ShravanSunder/MyRepo".to_owned())
    );
    assert!(NormalizedOrigin::try_from("GitHub.com/owner/repo".to_owned()).is_err());
}

#[test]
fn distinct_resource_id_types_do_not_interchange() {
    let _board_id = BoardId::try_from(id_string(1)).expect("board ID");
    let _topic_id = TopicId::try_from(id_string(2)).expect("topic ID");
    let _project_id = ProjectId::try_from(id_string(3)).expect("project ID");
}

#[test]
fn every_public_operation_has_a_typed_schema_contract() {
    fn assert_contract<T>()
    where
        T: serde::Serialize + serde::de::DeserializeOwned + schemars::JsonSchema,
    {
    }
    macro_rules! contracts {
        ($($contract:ty),+ $(,)?) => { $(assert_contract::<$contract>();)+ };
    }
    use project_board::*;
    contracts!(
        ProjectCreateRequest,
        ProjectCreateResult,
        ProjectUpdateRequest,
        ProjectUpdateResult,
        ProjectShowRequest,
        ProjectShowResult,
        ProjectListRequest,
        ProjectListResult,
        RepositoryAttachRequest,
        RepositoryAttachResult,
        RepositoryDetachRequest,
        RepositoryDetachResult,
        RepositoryListRequest,
        RepositoryListResult,
        BoardCreateRequest,
        BoardCreateResult,
        BoardUpdateRequest,
        BoardUpdateResult,
        BoardShowRequest,
        BoardShowResult,
        BoardListRequest,
        BoardListResult,
        BoardArchiveRequest,
        BoardArchiveResult,
        TopicCreateRequest,
        TopicCreateResult,
        TopicUpdateRequest,
        TopicUpdateResult,
        TopicListRequest,
        TopicListResult,
        MessagePostRequest,
        MessagePostResult,
        MessageShowRequest,
        MessageShowResult,
        MessageListRequest,
        MessageListResult,
        ThreadShowRequest,
        ThreadShowResult,
        ThreadResolveRequest,
        ThreadResolveResult,
        ThreadUnresolveRequest,
        ThreadUnresolveResult,
        ThreadWatchRequest,
        ThreadWatchResult,
        ThreadUnwatchRequest,
        ThreadUnwatchResult,
        ThreadListRequest,
        ThreadListResult,
        InboxFetchRequest,
        InboxFetchResult,
        InboxAcknowledgeRequest,
        InboxAcknowledgeResult,
        InboxProjectsRequest,
        InboxProjectsResult,
    );
}

#[test]
fn actionable_failure_factories_preserve_closed_details() {
    use project_board::{BoardError, BoardErrorDetails, BoardFailureKind, BoardNextAction};
    let failure = BoardError::top_level_message_cooldown(7);
    assert_eq!(failure.kind, BoardFailureKind::TopLevelMessageCooldown);
    assert_eq!(failure.next_action, BoardNextAction::PostThreadMessage);
    assert_eq!(
        failure.details,
        BoardErrorDetails::Cooldown {
            retry_after_seconds: 7
        }
    );
    assert!(failure.message.contains("Wait 7 seconds"));
    assert_eq!(
        BoardError::thread_resolved().message,
        "This thread is resolved. Mark it unresolved before adding a thread message."
    );
}
