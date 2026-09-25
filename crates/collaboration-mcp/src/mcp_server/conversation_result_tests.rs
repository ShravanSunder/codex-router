//! Common conversation completion and failure projection tests.
use collaboration_protocol::OperationId;

#[test]
fn create_and_prompt_failure_retains_created_target_and_unknown_effect() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"created-thread"
    }))
    .expect("target");
    let create_operation_id = OperationId::generate();
    let result =
        super::conversation_tool_result::<collaboration_client::ConversationCreatePromptOutcome>(
            Err(collaboration_client::ConversationClientError::AfterCreate {
                create_operation_id: create_operation_id.clone(),
                target: target.clone(),
                source: Box::new(collaboration_client::ConversationClientError::from(
                    collaboration_client::OperationError::after_dispatch(
                        "prompt",
                        Some(target.clone()),
                        None,
                        collaboration_client::ClientError::Rejected {
                            code: -32603,
                            data: Some(serde_json::json!({"reason":"prompt rejected"})),
                        },
                    ),
                )),
            }),
            Some(create_operation_id.clone()),
        );
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["target"], serde_json::json!(target));
    assert_eq!(
        structured["createOperationId"],
        serde_json::json!(create_operation_id)
    );
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["kind"], "rejected");
}

#[test]
fn resumed_prompt_failure_retains_known_target_and_rejection_evidence() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"existing-thread"
    }))
    .expect("target");
    let result = super::common_prompt_tool_result(Err(
        collaboration_client::OperationError::after_dispatch(
            "prompt",
            Some(target.clone()),
            None,
            collaboration_client::ClientError::Rejected {
                code: -32603,
                data: Some(serde_json::json!({"kind":"nativeRejected","detail":"busy"})),
            },
        ),
    ));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["target"], serde_json::json!(target));
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["kind"], "rejected");
    assert_eq!(structured["serviceKind"], "nativeRejected");
    assert_eq!(structured["code"], -32603);
    assert_eq!(structured["data"]["detail"], "busy");
}

#[test]
fn prompt_carrier_preserves_permission_required_and_explicit_approver() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"created-thread"
    }))
    .expect("target");
    let approver: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"approver-thread"
    }))
    .expect("approver");
    let create_operation_id = OperationId::generate();
    let request: collaboration_client::ConversationCreatePromptInput =
        serde_json::from_value(serde_json::json!({
            "create":{
                "operationId":create_operation_id,"endpoint":target.endpoint,
                "workingDirectory":"/tmp/collaboration-mcp-fixture",
                "access":"workspace-write","createdBy":approver,"approver":approver,
                "model":"gpt-5.6-sol","effort":"low"
            },
            "message":{"kind":"humanUser","text":"permission fixture"},
            "promptEffort":"low"
        }))
        .expect("common create-and-prompt input");
    let encoded = serde_json::to_value(&request).expect("request encoding");
    assert_eq!(encoded["create"]["approver"], serde_json::json!(approver));

    let outcome: collaboration_client::ConversationCreatePromptOutcome =
        serde_json::from_value(serde_json::json!({
            "kind":"prompt","createOperationId":create_operation_id,
            "prompt":{"kind":"completed","target":target,"settlement":{
                "target":target,"stopReason":"cancelled","detail":{
                    "kind":"codexPrompt","updates":[],"permissionRequired":true,
                    "result":{"stopReason":"cancelled"}
                }
            }}
        }))
        .expect("common prompt settlement");
    let result = super::conversation_tool_result(Ok(outcome), Some(create_operation_id));
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured result");
    assert_eq!(
        structured["prompt"]["settlement"]["detail"]["permissionRequired"],
        true
    );
    assert_eq!(
        structured["prompt"]["settlement"]["stopReason"],
        "cancelled"
    );
}

#[test]
fn prompt_carrier_preserves_busy_precondition_rejection_without_auto_retry() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"busy-thread"
    }))
    .expect("target");
    let result = super::common_prompt_tool_result(Err(
        collaboration_client::OperationError::after_dispatch(
            "prompt",
            Some(target),
            None,
            collaboration_client::ClientError::Rejected {
                code: -32050,
                data: Some(serde_json::json!({
                    "kind":"nativeRejected",
                    "stage":"prompt",
                    "reason":"busy",
                    "nextAction":"inspectTarget"
                })),
            },
        ),
    ));
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured rejection");
    assert_eq!(structured["kind"], "rejected");
    assert_eq!(structured["serviceKind"], "nativeRejected");
    assert_eq!(structured["stage"], "prompt");
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["data"]["reason"], "busy");
    assert_eq!(structured["data"]["nextAction"], "inspectTarget");
}

#[test]
fn resumed_prompt_preflight_failure_retains_requested_target_without_effect() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"existing-thread"
    }))
    .expect("target");
    let result = super::common_prompt_tool_result(Err(
        collaboration_client::OperationError::before_dispatch(
            "validation",
            Some(target.clone()),
            collaboration_client::ClientError::InvalidRequest(
                "prompt timeout must be at least one second",
            ),
        ),
    ));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["target"], serde_json::json!(target));
    assert_eq!(structured["effect"], "none");
    assert_eq!(structured["kind"], "protocolViolation");
    assert_eq!(structured["stage"], "validation");
}

#[test]
fn resumed_prompt_load_response_loss_retains_target_with_unknown_effect() {
    let target: collaboration_protocol::SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"existing-thread"
    }))
    .expect("target");
    let result = super::common_prompt_tool_result(Err(
        collaboration_client::OperationError::after_dispatch(
            "load",
            Some(target.clone()),
            None,
            collaboration_client::ClientError::Transport(std::io::Error::new(
                std::io::ErrorKind::ConnectionReset,
                "fixture",
            )),
        ),
    ));
    let structured = result.structured_content.expect("structured error");
    assert_eq!(structured["target"], serde_json::json!(target));
    assert_eq!(structured["effect"], "unknown");
    assert_eq!(structured["kind"], "unavailable");
    assert_eq!(structured["stage"], "load");
    assert_eq!(structured["data"]["ioKind"], "ConnectionReset");
}
