use super::{
    ConversationClient, ConversationClientError, ConversationCreateInput, ConversationTransport,
    advertised_conversation_transport,
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

#[tokio::test]
async fn provider_create_rejects_codex_only_inputs_before_mutation() {
    use collaboration_service::{ServiceIdentity, serve_control_connection};
    for field in ["model", "effort", "fork", "rootMessageId"] {
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("socket pair: {error}"));
        let identity = ServiceIdentity::new(
            "019f0000-0000-7000-8000-000000000001",
            "019f0000-0000-7000-8000-000000000002",
            &format!("sha256:{}", "a".repeat(64)),
        )
        .unwrap_or_else(|error| panic!("service identity: {error}"));
        let serving = tokio::spawn(serve_control_connection(server, identity));
        let control = crate::ControlClient::initialize(client, "conversation-route-test", "1")
            .await
            .unwrap_or_else(|error| panic!("initialize: {error}"));
        let mut input: ConversationCreateInput = serde_json::from_value(json!({
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"claude-local"},
            "workingDirectory":"/tmp/project","access":"workspace-write",
            "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
        })).unwrap_or_else(|error| panic!("create input: {error}"));
        match field {
            "model" => input.model = Some("gpt-6-sol".to_owned()),
            "effort" => input.effort = Some("medium".to_owned()),
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
        let result = ConversationClient::ExternalProvider(control)
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
        serving
            .await
            .unwrap_or_else(|error| panic!("service join: {error}"))
            .unwrap_or_else(|error| panic!("service result: {error}"));
    }
}

#[tokio::test]
async fn provider_create_wait_returns_the_target_from_exact_operation()
-> Result<(), Box<dyn std::error::Error>> {
    use collaboration_protocol::{
        ConversationAdmissionState, ConversationCancelRequest, ConversationCreateOutcome,
        ConversationCreateRequest as ProviderCreateRequest, ConversationLoadRequest,
        ConversationOperationReconcileRequest, ConversationOperationShowRequest,
        ConversationOperationSnapshot, ConversationOperationSubmission,
        ConversationOperationWaitOutput, ConversationOperationWaitRequest,
        ConversationOperationWaitResult, ConversationPromptRequest, EndpointRef,
        ProviderBindingIdentity,
    };
    use collaboration_service::{
        ProviderConversationBackend, ProviderConversationFuture, ServiceIdentity,
        serve_control_connection,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[derive(Clone)]
    struct CreateBackend {
        binding: ProviderBindingIdentity,
        admitted: ConversationOperationSnapshot,
        settled: ConversationOperationSnapshot,
        create_calls: Arc<AtomicUsize>,
        wait_calls: Arc<AtomicUsize>,
        wait_delay: std::time::Duration,
    }
    impl ProviderConversationBackend for CreateBackend {
        fn binding(&self, endpoint: &EndpointRef) -> Option<ProviderBindingIdentity> {
            (&self.binding.endpoint == endpoint).then(|| self.binding.clone())
        }
        fn create(
            &self,
            _: ProviderCreateRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
            self.create_calls.fetch_add(1, Ordering::SeqCst);
            let operation = self.admitted.clone();
            Box::pin(async move {
                Ok(ConversationOperationSubmission {
                    admission: ConversationAdmissionState::Admitted,
                    operation,
                })
            })
        }
        fn load(
            &self,
            _: ConversationLoadRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
            let operation = self.admitted.clone();
            Box::pin(async move {
                Ok(ConversationOperationSubmission {
                    admission: ConversationAdmissionState::Admitted,
                    operation,
                })
            })
        }
        fn prompt(
            &self,
            _: ConversationPromptRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
            let operation = self.admitted.clone();
            Box::pin(async move {
                Ok(ConversationOperationSubmission {
                    admission: ConversationAdmissionState::Admitted,
                    operation,
                })
            })
        }
        fn cancel(
            &self,
            _: ConversationCancelRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSubmission> {
            let operation = self.admitted.clone();
            Box::pin(async move {
                Ok(ConversationOperationSubmission {
                    admission: ConversationAdmissionState::Admitted,
                    operation,
                })
            })
        }
        fn show(
            &self,
            _: ConversationOperationShowRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
            let operation = self.settled.clone();
            Box::pin(async move { Ok(operation) })
        }
        fn wait(
            &self,
            _: ConversationOperationWaitRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationWaitResult> {
            self.wait_calls.fetch_add(1, Ordering::SeqCst);
            let operation = self.settled.clone();
            let wait_delay = self.wait_delay;
            Box::pin(async move {
                tokio::time::sleep(wait_delay).await;
                Ok(ConversationOperationWaitResult {
                    operation,
                    output: ConversationOperationWaitOutput::OutputUnavailable {
                        reason:
                            collaboration_protocol::ConversationOutputUnavailableReason::NotRetained,
                    },
                })
            })
        }
        fn reconcile(
            &self,
            _: ConversationOperationReconcileRequest,
        ) -> ProviderConversationFuture<'_, ConversationOperationSnapshot> {
            let operation = self.settled.clone();
            Box::pin(async move { Ok(operation) })
        }
    }

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
    let create_calls = Arc::new(AtomicUsize::new(0));
    let wait_calls = Arc::new(AtomicUsize::new(0));
    let backend = CreateBackend {
        binding: binding.clone(),
        admitted: snapshot("admitted", "none", None)?,
        settled: snapshot("terminal", "applied", Some(target.clone()))?,
        create_calls: Arc::clone(&create_calls),
        wait_calls: Arc::clone(&wait_calls),
        wait_delay: std::time::Duration::ZERO,
    };
    let identity = ServiceIdentity::new(
        "019f0000-0000-7000-8000-000000000001",
        "019f0000-0000-7000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_provider_conversation_backend(Arc::new(backend));
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_control_connection(server, identity));
    let control = crate::ControlClient::initialize(client, "conversation-route-test", "1").await?;
    let input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":operation_id,"endpoint":binding.endpoint,"workingDirectory":"/tmp/project",
        "access":"workspace-write",
        "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
    }))?;
    let outcome = ConversationClient::ExternalProvider(control)
        .create(input, std::time::Duration::from_secs(1))
        .await?;
    if outcome
        != (ConversationCreateOutcome::Created {
            operation_id: operation_id.clone(),
            target: serde_json::from_value(target)?,
        })
        || create_calls.load(Ordering::SeqCst) != 1
        || wait_calls.load(Ordering::SeqCst) != 1
    {
        return Err(format!(
            "provider create did not settle from its exact operation: {outcome:?}"
        )
        .into());
    }
    serving.await??;

    let delayed_backend = CreateBackend {
        binding: binding.clone(),
        admitted: snapshot("admitted", "none", None)?,
        settled: snapshot("terminal", "applied", None)?,
        create_calls: Arc::new(AtomicUsize::new(0)),
        wait_calls: Arc::new(AtomicUsize::new(0)),
        wait_delay: std::time::Duration::from_secs(2),
    };
    let identity = ServiceIdentity::new(
        "019f0000-0000-7000-8000-000000000001",
        "019f0000-0000-7000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_provider_conversation_backend(Arc::new(delayed_backend.clone()));
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_control_connection(server, identity));
    let control =
        crate::ControlClient::initialize(client, "conversation-timeout-test", "1").await?;
    let input: ConversationCreateInput = serde_json::from_value(json!({
        "operationId":operation_id,"endpoint":binding.endpoint,"workingDirectory":"/tmp/project",
        "access":"workspace-write",
        "createdBy":{"endpoint":{"serviceId":"019f0000-0000-7000-8000-000000000001","endpointId":"codex-local"},"sessionId":"caller"}
    }))?;
    let pending = ConversationClient::ExternalProvider(control)
        .create(input, std::time::Duration::from_secs(1))
        .await?;
    if pending != (ConversationCreateOutcome::Pending { operation_id })
        || delayed_backend.create_calls.load(Ordering::SeqCst) != 1
        || delayed_backend.wait_calls.load(Ordering::SeqCst) != 1
    {
        return Err(
            format!("timed-out create lost its admitted operation identity: {pending:?}").into(),
        );
    }
    serving.await??;
    Ok(())
}
