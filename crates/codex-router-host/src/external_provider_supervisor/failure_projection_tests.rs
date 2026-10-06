use super::*;

#[test]
fn lost_provider_prompt_has_terminal_unknown_effect_and_sanitized_reason() {
    // A prompt dispatched before connection loss cannot be retried safely.
    // Specification E4 and R5 require a terminal lost projection in PR 1.
    let failure = prompt_runtime_failure(
        OperationId::generate(),
        None,
        ExternalProviderRuntimeError::TransportFailure,
    );
    assert_eq!(
        failure.kind,
        ConversationOperationFailureKind::OutcomeUnknown
    );
    assert_eq!(failure.effect, ProviderOperationEffect::Unknown);
    assert_eq!(
        String::from(failure.message),
        "provider connection lost before the agent ended the turn (providerRetired)"
    );
}

#[test]
fn unsupported_prompt_content_is_a_validation_failure_without_provider_effect() {
    // ACP v1 initialization.mdx:202-217 makes optional prompt content
    // conditional on advertised capabilities.
    for (content_type, expected_message) in [
        ("image", "unsupportedContent{image}"),
        ("audio", "unsupportedContent{audio}"),
        ("embeddedResource", "unsupportedContent{embeddedResource}"),
    ] {
        let failure = runtime_failure(
            OperationId::generate(),
            None,
            ExternalProviderRuntimeError::UnsupportedContent { content_type },
        );
        assert_eq!(
            failure.kind,
            ConversationOperationFailureKind::UnsupportedCapability
        );
        assert_eq!(failure.stage, ConversationOperationFailureStage::Validation);
        assert_eq!(failure.effect, ProviderOperationEffect::None);
        assert_eq!(String::from(failure.message), expected_message);
    }
}

#[tokio::test]
async fn unknown_agent_stop_reason_settles_with_typed_value() {
    // ACP v1 prompt-turn.mdx:369-390 defines the recognized stop reasons.
    // R4 preserves an unknown value in the terminal settlement.
    let fixture = crate::external_provider_runtime::acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "unknown-stop-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "fixture-session"}))
        .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "fixture-session"}))
        .respond("prompt", serde_json::json!({"stopReason": "future_reason"}))
        .launch();
    let root = tempfile::tempdir().expect("temporary root");
    let runtime = ExternalProviderRuntime::initialize(fixture)
        .await
        .expect("fixture initializes");
    runtime
        .create_session(root.path().to_owned())
        .await
        .expect("session created");
    let store = Arc::new(Mutex::new(
        ProviderOperationStore::open(&root.path().join("operations.sqlite"))
            .await
            .expect("operation store"),
    ));
    let backend = ExternalProviderSupervisor::new(
        vec![ExternalProviderBinding {
            identity: binding(),
            runtime,
        }],
        store,
    )
    .expect("supervisor");
    let operation_id = OperationId::generate();
    assert_eq!(
        backend
            .submit_delivery_prompt_contents(prepared_prompt_request(PreparedPromptFixture {
                operation_id: operation_id.clone(),
                input_id: session_event_model::InputId::generate(),
                target: SessionRef {
                    endpoint: endpoint(),
                    session_id: SessionId::try_from("fixture-session".to_owned()).expect("session"),
                },
                preview: "continue".to_owned(),
            }))
            .await
            .expect("prompt submitted"),
        provider_delivery_submission::ProviderPromptDispatch::Submitted
    );
    let waited = backend
        .wait(ConversationOperationWaitRequest {
            operation_id: operation_id.clone(),
            timeout_seconds: PositiveSeconds::try_from(5).expect("timeout"),
        })
        .await
        .expect("unknown stop reason is a typed terminal settlement");
    assert!(matches!(waited.output,
        ConversationOperationWaitOutput::Available {
            settlement: ConversationOperationSettlement::PromptCompleted {
                stop_reason: ProviderPromptStopReason::Unknown(value), ..
            }
        } if value == "future_reason"));
    let operation = backend
        .show(ConversationOperationShowRequest { operation_id })
        .await
        .expect("terminal operation");
    assert_eq!(operation.stage, ProviderOperationStage::Terminal);
    assert_eq!(operation.effect, ProviderOperationEffect::Applied);
    assert_eq!(operation.terminal_stop_reason, None);
    backend.shutdown().await.expect("supervisor shutdown");
}

#[test]
fn provider_session_not_found_failure_has_typed_guidance_and_code() {
    let correlation_id = acp_client_runtime::ProviderErrorCorrelationId::generate();
    let correlation_reference = correlation_id.to_string();
    let failure = runtime_failure(
        OperationId::generate(),
        None,
        ExternalProviderRuntimeError::ProviderSessionNotFound {
            code: -32002,
            correlation_id,
        },
    );

    assert_eq!(
        failure.kind,
        ConversationOperationFailureKind::ProviderSessionNotFound
    );
    assert_eq!(failure.provider_code, Some(-32002));
    assert_eq!(
        String::from(failure.message),
        format!(
            "this session never started a turn and did not survive the provider restart; create a new conversation (provider code -32002; reference {correlation_reference})"
        )
    );
}
