use super::{
    ConversationClient, ConversationClientError, ConversationCreateInput, ConversationTransport,
    advertised_conversation_transport, codex_create_actors,
};
use collaboration_protocol::EndpointDescription;
use serde_json::json;

#[test]
fn provider_prompt_and_load_settlements_keep_exact_operation_and_client_evidence() {
    use crate::conversation_session_operations::{
        ProviderSettlementKind, provider_operation_result,
    };
    use crate::{
        ConversationOperationResult, ConversationSettlementDetail, ConversationStopReason,
        ProviderLoadOutput, ProviderPromptOutput,
    };
    use collaboration_protocol::{ConversationOperationWaitResult, OperationId, SessionRef};

    let operation_id = OperationId::generate();
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
        "sessionId":"provider-thread"
    }))
    .expect("target");
    let snapshot = |operation: &str, stage: &str| {
        json!({
            "operationId":operation_id,"operation":operation,
            "binding":{"kind":"externalProvider","binding":{
                "endpoint":target.endpoint,"bindingId":"fixture-binding",
                "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
                "transport":"stdioAcp",
                "generation":{"serviceEpoch":"019f0000-0000-7000-8000-000000000002","generation":1},
                "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
            }},
            "target":target,"stage":stage,"effect":if stage == "terminal" {"applied"} else {"unknown"},
            "reconciliation":if stage == "terminal" {"confirmed"} else {"unresolved"},
            "admittedAt":"2026-09-24T00:00:00Z",
            "terminalAt":if stage == "terminal" {Some("2026-09-24T00:00:01Z")} else {None}
        })
    };
    let prompt: ConversationOperationWaitResult = serde_json::from_value(json!({
        "operation":snapshot("conversationPrompt", "terminal"),
        "output":{"kind":"available","settlement":{"kind":"promptCompleted",
            "target":target,"stopReason":"end_turn","response":"provider answer"}}
    }))
    .expect("prompt settlement");
    let result = provider_operation_result(
        prompt,
        operation_id.clone(),
        target.clone(),
        ProviderSettlementKind::Prompt,
    )
    .expect("completed prompt");
    match result {
        ConversationOperationResult::Completed {
            target: returned,
            operation_id: Some(returned_id),
            settlement,
        } => {
            assert_eq!(returned, target);
            assert_eq!(returned_id, operation_id);
            assert_eq!(
                settlement.stop_reason,
                Some(ConversationStopReason::EndTurn)
            );
            assert!(matches!(
                settlement.detail,
                ConversationSettlementDetail::ProviderPrompt {
                    output: ProviderPromptOutput::Available { text: Some(_) }
                }
            ));
        }
        _ => panic!("prompt settlement identity or detail was lost"),
    }

    let load: ConversationOperationWaitResult = serde_json::from_value(json!({
        "operation":snapshot("conversationLoad", "terminal"),
        "output":{"kind":"available","settlement":{"kind":"loaded","target":target,
            "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},
                "mappingStatus":"verified","authentication":"authenticated"}}}
    }))
    .expect("load settlement");
    let result = provider_operation_result(
        load,
        operation_id.clone(),
        target.clone(),
        ProviderSettlementKind::Load,
    )
    .expect("completed load");
    match result {
        ConversationOperationResult::Completed {
            operation_id: Some(returned_id),
            settlement,
            ..
        } => {
            assert_eq!(returned_id, operation_id);
            assert!(settlement.stop_reason.is_none());
            assert!(matches!(
                settlement.detail,
                ConversationSettlementDetail::ProviderLoad {
                    output: ProviderLoadOutput::Available { .. }
                }
            ));
        }
        _ => panic!("load settlement identity or detail was lost"),
    }

    let pending: ConversationOperationWaitResult = serde_json::from_value(json!({
        "operation":snapshot("conversationPrompt", "mayHaveDispatched"),
        "output":{"kind":"pending"}
    }))
    .expect("pending operation");
    assert!(matches!(
        provider_operation_result(pending, operation_id.clone(), target.clone(), ProviderSettlementKind::Prompt),
        Ok(ConversationOperationResult::Pending { operation_id: returned_id, .. }) if returned_id == operation_id
    ));

    for (operation, expected) in [
        ("conversationPrompt", ProviderSettlementKind::Prompt),
        ("conversationLoad", ProviderSettlementKind::Load),
    ] {
        let unavailable: ConversationOperationWaitResult = serde_json::from_value(json!({
            "operation":snapshot(operation, "terminal"),
            "output":{"kind":"outputUnavailable","reason":"notRetained"}
        }))
        .expect("applied operation without retained output");
        let result =
            provider_operation_result(unavailable, operation_id.clone(), target.clone(), expected)
                .expect("applied operation remains completed");
        match result {
            ConversationOperationResult::Completed {
                operation_id: Some(returned_id),
                settlement,
                ..
            } => {
                assert_eq!(returned_id, operation_id);
                assert_eq!(settlement.target, target);
                assert!(settlement.stop_reason.is_none());
                assert!(matches!(
                    settlement.detail,
                    ConversationSettlementDetail::ProviderPrompt {
                        output: ProviderPromptOutput::Unavailable { .. }
                    } | ConversationSettlementDetail::ProviderLoad {
                        output: ProviderLoadOutput::Unavailable { .. }
                    }
                ));
            }
            _ => panic!("applied operation with unavailable output was not completed"),
        }
    }
}

fn endpoint(channels: serde_json::Value) -> EndpointDescription {
    serde_json::from_value(json!({
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"fixture-local"},
        "label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":channels
    })).unwrap_or_else(|error| panic!("valid endpoint fixture: {error}"))
}

#[test]
fn conversation_create_defaults_approver_to_the_caller() {
    let mut input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":collaboration_protocol::OperationId::generate(),
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
        "workingDirectory":"/tmp/project","access":"workspace-write",
        "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
    }))
    .unwrap_or_else(|error| panic!("create input: {error}"));
    let caller = input.created_by.clone();

    input.default_approver();

    assert_eq!(input.approver, Some(caller));
}

#[test]
fn typed_human_create_is_provider_only_and_legacy_session_json_remains_valid() {
    let mut input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":collaboration_protocol::OperationId::generate(),
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
        "workingDirectory":"/tmp/project","access":"workspace-write",
        "createdBy":{"kind":"human","humanId":"fixture-owner"},
        "approver":{"kind":"human","humanId":"fixture-owner"}
    }))
    .unwrap_or_else(|error| panic!("typed Human create input: {error}"));
    assert!(
        ConversationClient::validate_create_input(&input, std::time::Duration::from_secs(1))
            .is_ok()
    );
    assert!(matches!(
        codex_create_actors(&input),
        Err(ConversationClientError::UnsupportedInput { field: "createdBy", fix, .. })
            if fix.contains("provider endpoints only")
    ));
    let session = json!({
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},
        "sessionId":"caller"
    });
    input.created_by = serde_json::from_value(session.clone())
        .unwrap_or_else(|error| panic!("legacy SessionRef create actor: {error}"));
    assert!(matches!(
        codex_create_actors(&input),
        Err(ConversationClientError::UnsupportedInput { field: "approver", fix, .. })
            if fix.contains("provider endpoints only")
    ));
    input.approver = Some(
        serde_json::from_value(session)
            .unwrap_or_else(|error| panic!("legacy SessionRef approver: {error}")),
    );
    assert!(codex_create_actors(&input).is_ok());
}

#[test]
fn conversation_transport_follows_advertised_channel() {
    let acp = endpoint(json!([
        {"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null},
        {"kind":"acp","transport":"unixJsonLines","path":"codex-acp.sock","schemaDigest":format!("sha256:{}", "a".repeat(64))}
    ]));
    assert_eq!(
        advertised_conversation_transport(&acp).ok(),
        Some(ConversationTransport::CodexAcp)
    );

    let provider = endpoint(json!([{"kind":"externalProvider","transport":"stdioAcp",
        "bindingId":"fixture-binding","bindingGeneration":1,
        "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
        "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]
    }]));
    assert_eq!(
        advertised_conversation_transport(&provider).ok(),
        Some(ConversationTransport::ExternalProvider)
    );

    let native_only = endpoint(json!([{"kind":"nativeCodex","transport":"unixWebSocket",
        "path":"codex-native.sock","schemaDigest":null,"generation":null
    }]));
    assert!(advertised_conversation_transport(&native_only).is_err());
}

/// The Router's provider operations as scripted answers, counting what reached them.
struct ScriptedProviderRouter {
    admitted: Option<collaboration_protocol::ConversationOperationSnapshot>,
    settled: Option<collaboration_protocol::ConversationOperationSnapshot>,
    calls: std::sync::atomic::AtomicUsize,
    create_calls: std::sync::atomic::AtomicUsize,
    wait_calls: std::sync::atomic::AtomicUsize,
    wait_delay: std::time::Duration,
}

impl ScriptedProviderRouter {
    fn unreachable() -> Self {
        Self {
            admitted: None,
            settled: None,
            calls: std::sync::atomic::AtomicUsize::new(0),
            create_calls: std::sync::atomic::AtomicUsize::new(0),
            wait_calls: std::sync::atomic::AtomicUsize::new(0),
            wait_delay: std::time::Duration::ZERO,
        }
    }

    fn admitted(
        &self,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationSubmission> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let operation = self.admitted.clone();
        Box::pin(async move {
            Ok(collaboration_protocol::ConversationOperationSubmission {
                admission: collaboration_protocol::ConversationAdmissionState::Admitted,
                operation: operation.ok_or(crate::ClientError::Protocol("no provider I/O"))?,
            })
        })
    }
}

impl crate::LocalCollaboration for ScriptedProviderRouter {
    fn service_id(&self) -> collaboration_protocol::UuidIdentity {
        "019f0000-0000-7000-8000-000000000001"
            .to_owned()
            .try_into()
            .expect("service identity")
    }
    fn endpoints(&self) -> Result<collaboration_protocol::EndpointInventory, crate::ClientError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Err(crate::ClientError::Protocol("unused endpoint inventory"))
    }
    fn create_provider_conversation(
        &self,
        _request: collaboration_protocol::ConversationCreateRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationSubmission> {
        self.create_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.admitted()
    }
    fn load_provider_conversation(
        &self,
        _request: collaboration_protocol::ConversationLoadRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationSubmission> {
        self.admitted()
    }
    fn prompt_provider_conversation(
        &self,
        _request: collaboration_protocol::ConversationPromptRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationSubmission> {
        self.admitted()
    }
    fn cancel_provider_conversation_operation(
        &self,
        _request: collaboration_protocol::ConversationCancelRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationSubmission> {
        self.admitted()
    }
    fn wait_for_provider_conversation_operation(
        &self,
        _request: collaboration_protocol::ConversationOperationWaitRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::ConversationOperationWaitResult> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.wait_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let operation = self.settled.clone();
        let wait_delay = self.wait_delay;
        Box::pin(async move {
            tokio::time::sleep(wait_delay).await;
            let operation = operation.ok_or(crate::ClientError::Protocol("no provider I/O"))?;
            let target = operation.target.clone();
            Ok(collaboration_protocol::ConversationOperationWaitResult {
                operation,
                output: collaboration_protocol::ConversationOperationWaitOutput::Available {
                    settlement: serde_json::from_value(json!({
                        "kind":"created","target":target,
                        "effectiveSettings":{"requestedPolicy":{"access":"workspace-write"},
                            "mappingStatus":"verified","authentication":"authenticated"}
                    }))
                    .expect("created settlement"),
                },
            })
        })
    }
    fn observe_provider_session(
        &self,
        _request: collaboration_protocol::BoundedObservationRequest,
    ) -> crate::LocalFuture<'_, collaboration_protocol::BoundedObservationResult> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Err(crate::ClientError::Protocol("unused observation")) })
    }
}

#[tokio::test]
async fn provider_create_rejects_codex_only_inputs_before_mutation() {
    use std::sync::Arc;
    for field in ["fork", "rootMessageId"] {
        let router = Arc::new(ScriptedProviderRouter::unreachable());
        let mut input: ConversationCreateInput = serde_json::from_value(json!({
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
            "workingDirectory":"/tmp/project","access":"workspace-write",
            "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
        })).unwrap_or_else(|error| panic!("create input: {error}"));
        match field {
            "fork" => {
                input.fork = Some(
                    "source-thread"
                        .to_owned()
                        .try_into()
                        .unwrap_or_else(|error| panic!("fork ID: {error}")),
                )
            }
            "rootMessageId" => {
                input.root_message_id = Some(
                    "019f0000-0000-7000-8000-000000000099"
                        .to_owned()
                        .try_into()
                        .unwrap_or_else(|error| panic!("root message ID: {error}")),
                )
            }
            _ => panic!("unexpected unsupported field fixture: {field}"),
        }
        let result = ConversationClient::ExternalProvider(crate::ProviderConversations::Local(
            Arc::clone(&router) as Arc<dyn crate::LocalCollaboration>,
        ))
        .create(input, std::time::Duration::from_secs(1))
        .await;
        match result {
            Err(ConversationClientError::UnsupportedInput {
                endpoint,
                field: rejected,
                fix,
            }) => {
                assert_eq!(rejected, field);
                assert_eq!(String::from(endpoint.endpoint_id), "claude-local");
                assert!(fix.contains("claude-local"));
            }
            _ => panic!("{field} was not rejected before provider I/O"),
        }
        assert_eq!(
            router.calls.load(std::sync::atomic::Ordering::SeqCst)
                + router
                    .create_calls
                    .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "{field} reached the Router"
        );
    }
}

#[tokio::test]
async fn provider_create_wait_returns_the_target_from_exact_operation()
-> Result<(), Box<dyn std::error::Error>> {
    use collaboration_protocol::{
        ConversationCreateOutcome, ConversationOperationSnapshot, ProviderBindingIdentity,
    };
    use std::sync::{Arc, atomic::Ordering};

    let operation_id = collaboration_protocol::OperationId::generate();
    let binding: ProviderBindingIdentity = serde_json::from_value(json!({
        "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
        "bindingId":"fixture-binding",
        "runtime":{"provider":"claudeCode","runtimeName":"fixture"},
        "transport":"stdioAcp",
        "generation":{"serviceEpoch":"019f0000-0000-7000-8000-000000000002","generation":1},
        "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]
    }))?;
    let target = json!({"endpoint":binding.endpoint,"sessionId":"provider-created"});
    let snapshot = |stage: &str,
                    effect: &str,
                    target: Option<serde_json::Value>|
     -> Result<ConversationOperationSnapshot, serde_json::Error> {
        serde_json::from_value(json!({
            "operationId":operation_id,"operation":"conversationCreate",
            "binding":{"kind":"externalProvider","binding":binding},
            "target":target,"stage":stage,"effect":effect,"reconciliation":"confirmed",
            "admittedAt":"2026-09-24T00:00:00Z","terminalAt":if stage == "terminal" {Some("2026-09-24T00:00:01Z")} else {None}
        }))
    };
    let router = Arc::new(ScriptedProviderRouter {
        admitted: Some(snapshot("admitted", "none", None)?),
        settled: Some(snapshot("terminal", "applied", Some(target.clone()))?),
        ..ScriptedProviderRouter::unreachable()
    });
    let input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":operation_id,"endpoint":binding.endpoint,"workingDirectory":"/tmp/project",
        "access":"workspace-write",
        "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
    }))?;
    let outcome = ConversationClient::ExternalProvider(crate::ProviderConversations::Local(
        Arc::clone(&router) as Arc<dyn crate::LocalCollaboration>,
    ))
    .create(input, std::time::Duration::from_secs(1))
    .await?;
    if outcome
        != (ConversationCreateOutcome::Created {
            operation_id: operation_id.clone(),
            target: serde_json::from_value(target)?,
            effective_settings: Some(serde_json::from_value(json!({
                "requestedPolicy":{"access":"workspace-write"},
                "mappingStatus":"verified","authentication":"authenticated"
            }))?),
        })
        || router.create_calls.load(Ordering::SeqCst) != 1
        || router.wait_calls.load(Ordering::SeqCst) != 1
    {
        return Err(format!(
            "provider create did not settle from its exact operation: {outcome:?}"
        )
        .into());
    }

    let delayed_router = Arc::new(ScriptedProviderRouter {
        admitted: Some(snapshot("admitted", "none", None)?),
        settled: Some(snapshot("terminal", "applied", None)?),
        wait_delay: std::time::Duration::from_secs(2),
        ..ScriptedProviderRouter::unreachable()
    });
    let input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":operation_id,"endpoint":binding.endpoint,"workingDirectory":"/tmp/project",
        "access":"workspace-write",
        "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
    }))?;
    let pending = ConversationClient::ExternalProvider(crate::ProviderConversations::Local(
        Arc::clone(&delayed_router) as Arc<dyn crate::LocalCollaboration>,
    ))
    .create(input, std::time::Duration::from_secs(1))
    .await?;
    if pending != (ConversationCreateOutcome::Pending { operation_id })
        || delayed_router.create_calls.load(Ordering::SeqCst) != 1
        || delayed_router.wait_calls.load(Ordering::SeqCst) != 1
    {
        return Err(
            format!("timed-out create lost its admitted operation identity: {pending:?}").into(),
        );
    }
    Ok(())
}
