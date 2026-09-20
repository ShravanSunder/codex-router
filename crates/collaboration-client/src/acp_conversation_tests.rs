use super::*;
use collaboration_protocol::{EndpointId, MessageText};
use std::os::unix::fs::PermissionsExt;

fn endpoint(service_id: &str) -> EndpointRef {
    EndpointRef {
        service_id: UuidIdentity::try_from(service_id.to_owned()).expect("valid service UUID"),
        endpoint_id: EndpointId::try_from("codex-local".to_owned()).expect("valid endpoint"),
    }
}

#[test]
fn wrong_service_is_rejected_by_read_only_validation_before_session_open() {
    let observed = endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f");
    let requested = endpoint("018f47d2-24d5-7a68-b9ec-6f759c394590");
    let error = validate_conversation_endpoint(&observed, &requested)
        .expect_err("wrong service must be rejected");
    assert_eq!(
        error.to_string(),
        "invalid collaboration request: conversation endpoint belongs to another service"
    );
}

#[test]
fn public_prompt_schema_cannot_decode_router_authored_content() {
    let result = serde_json::from_value::<PublicPromptContent>(serde_json::json!({
        "kind": "router",
        "text": "internal"
    }));
    assert!(result.is_err());
}

#[test]
fn resumed_session_rejects_creation_settings_before_connection() {
    let request = ConversationCreateRequest {
        endpoint: endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f"),
        cwd: std::path::PathBuf::from("/tmp/project"),
        session: Some(SessionId::try_from("existing".to_owned()).expect("session ID")),
        fork: None,
        model: Some("gpt-5.6-luna".to_owned()),
        effort: None,
        access: None,
        created_by: None,
        approver: None,
        root_message_id: None,
    };
    assert!(matches!(
        validate_conversation_create_request(&request),
        Err(crate::ClientError::InvalidRequest(
            "existing session cannot change creation settings"
        ))
    ));
}

#[test]
fn new_and_forked_conversations_require_local_creator_and_approver_before_setup() {
    let local_endpoint = endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f");
    let foreign_endpoint = endpoint("018f47d2-24d5-7a68-b9ec-6f759c394590");
    let local_identity = SessionRef {
        endpoint: local_endpoint.clone(),
        session_id: SessionId::try_from("caller".to_owned()).expect("session ID"),
    };
    let foreign_identity = SessionRef {
        endpoint: foreign_endpoint,
        session_id: SessionId::try_from("foreign".to_owned()).expect("session ID"),
    };
    let request = |created_by, approver| ConversationCreateRequest {
        endpoint: local_endpoint.clone(),
        cwd: std::path::PathBuf::from("/tmp/project"),
        session: None,
        fork: None,
        model: Some("gpt-5.6-luna".to_owned()),
        effort: Some("low".to_owned()),
        access: Some("workspace-write".to_owned()),
        created_by,
        approver,
        root_message_id: None,
    };
    assert!(matches!(
        validate_conversation_create_request(&request(None, Some(local_identity.clone()))),
        Err(ClientError::InvalidRequest(
            "new or forked conversation requires createdBy"
        ))
    ));
    assert!(matches!(
        validate_conversation_create_request(&request(Some(local_identity.clone()), None)),
        Err(ClientError::InvalidRequest(
            "new or forked conversation requires approver"
        ))
    ));
    assert!(matches!(
        validate_conversation_create_request(&request(
            Some(foreign_identity),
            Some(local_identity.clone())
        )),
        Err(ClientError::InvalidRequest(
            "conversation identities belong to another endpoint"
        ))
    ));
    assert!(
        validate_conversation_create_request(&request(
            Some(local_identity.clone()),
            Some(local_identity)
        ))
        .is_ok()
    );
}

#[test]
fn existing_session_without_creation_identities_remains_valid() {
    let request = ConversationCreateRequest {
        endpoint: endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f"),
        cwd: std::path::PathBuf::from("/tmp/project"),
        session: Some(SessionId::try_from("existing".to_owned()).expect("session ID")),
        fork: None,
        model: None,
        effort: Some("medium".to_owned()),
        access: None,
        created_by: None,
        approver: None,
        root_message_id: None,
    };
    assert!(validate_conversation_create_request(&request).is_ok());
}

#[tokio::test]
async fn sdk_identity_preflight_fails_before_acp_discovery_or_mutation() {
    let local_endpoint = endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f");
    let foreign_endpoint = endpoint("018f47d2-24d5-7a68-b9ec-6f759c394590");
    let local_identity = SessionRef {
        endpoint: local_endpoint.clone(),
        session_id: SessionId::try_from("local".to_owned()).expect("session ID"),
    };
    let foreign_identity = SessionRef {
        endpoint: foreign_endpoint,
        session_id: SessionId::try_from("foreign".to_owned()).expect("session ID"),
    };
    for (created_by, approver) in [
        (None, Some(local_identity.clone())),
        (Some(foreign_identity), Some(local_identity.clone())),
    ] {
        let result = AcpConversation::create(
            std::path::Path::new("/path-that-must-not-be-read"),
            ConversationCreateRequest {
                endpoint: local_endpoint.clone(),
                cwd: std::path::PathBuf::from("/tmp/project"),
                session: None,
                fork: None,
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
                access: Some("workspace-write".to_owned()),
                created_by,
                approver,
                root_message_id: None,
            },
            &mut |_| Ok(()),
        )
        .await;
        let Err(error) = result else {
            panic!("identity preflight must precede discovery");
        };
        let (failure, target, turn_id) = error.into_parts();
        assert_eq!(failure.stage, "validation");
        assert_eq!(failure.effect, crate::OperationEffect::None);
        assert!(target.is_none());
        assert!(turn_id.is_none());
    }
}

#[test]
fn prompt_update_buffer_has_count_and_byte_limits() {
    let mut updates = BoundedPromptUpdates::new();
    updates.bytes = MAX_PROMPT_RESULT_BYTES;
    assert!(matches!(
        updates.push(serde_json::json!({"kind":"update"})),
        Err(crate::ClientError::Protocol("ACP prompt updates overflow"))
    ));
}

#[test]
fn resumed_prompt_failures_retain_the_known_target_for_rejection_and_post_dispatch_loss() {
    let target = SessionRef {
        endpoint: endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f"),
        session_id: SessionId::try_from("resumed-thread".to_owned()).expect("session ID"),
    };
    for source in [
        ClientError::Rejected {
            code: -32603,
            data: Some(serde_json::json!({"kind":"backendRejected"})),
        },
        ClientError::Transport(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "fixture EOF after dispatch",
        )),
    ] {
        let failure =
            crate::OperationError::after_dispatch("prompt", Some(target.clone()), None, source);
        let (_, failure_target, _) = failure.into_parts();
        assert_eq!(failure_target.as_ref(), Some(&target));
    }
}

#[tokio::test]
async fn invalid_prompt_deadline_fails_before_discovery_or_creation() {
    let endpoint = endpoint("018f47d2-24d5-7a68-b9ec-6f759c39458f");
    let sender = SessionRef {
        endpoint: endpoint.clone(),
        session_id: SessionId::try_from("sender".to_owned()).expect("session ID"),
    };
    let error = AcpConversation::create_and_prompt(
        std::path::Path::new("/path-that-must-not-be-read"),
        ConversationCreatePromptRequest {
            create: ConversationCreateRequest {
                endpoint,
                cwd: std::path::PathBuf::from("/tmp/project"),
                session: None,
                fork: None,
                model: Some("gpt-5.6-luna".to_owned()),
                effort: Some("low".to_owned()),
                access: Some("workspace-write".to_owned()),
                created_by: Some(sender.clone()),
                approver: Some(sender.clone()),
                root_message_id: None,
            },
            prompt: ConversationPromptRequest {
                message: PublicPromptContent::Agent {
                    sender,
                    text: MessageText::try_from("prompt".to_owned()).expect("message"),
                },
                effort: Some("low".to_owned()),
                timeout_seconds: 0,
            },
        },
        CancellationToken::new(),
    )
    .await
    .expect_err("invalid prompt timeout must fail before connection");
    let (failure, target, turn_id) = error.into_parts();
    assert_eq!(
        failure.message,
        "invalid collaboration request: prompt timeout must be at least one second"
    );
    assert_eq!(failure.effect, crate::OperationEffect::None);
    assert!(target.is_none());
    assert!(turn_id.is_none());
}

#[test]
fn only_write_restricted_access_creates_project_write_areas() {
    let root = std::env::temp_dir().join(format!("router-client-access-{}", uuid::Uuid::now_v7()));
    let restricted = root.join("restricted");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&restricted).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();

    prepare_project_write_areas(&restricted, Some("write-restricted")).unwrap();
    prepare_project_write_areas(&workspace, Some("workspace-write")).unwrap();

    assert!(restricted.join("tmp").is_dir());
    assert!(restricted.join("docs/wip").is_dir());
    assert!(!workspace.join("tmp").exists());
    assert!(!workspace.join("docs").exists());
}

#[test]
fn session_scratch_scopes_are_unique_and_owner_private() {
    let first = session_scratch_scope();
    let second = session_scratch_scope();
    assert_ne!(first, second);
    let path = std::env::temp_dir()
        .join("router-client-scratch")
        .join(first);
    assert!(create_private_scratch(&path).is_ok());
    let metadata = std::fs::metadata(path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o077, 0);
}
use crate::PublicPromptContent;
use collaboration_protocol::{SessionId, UuidIdentity};
