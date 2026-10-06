use super::*;

#[tokio::test]
async fn provider_option_sets_keep_order_scope_and_selected_id() -> TestResult {
    // PR 1 baseline: survey-cursor.md:54 and claude-adapter-and-sdk.md:153-155
    // describe these agent-offered choices. Extraction preserves their order,
    // disclosed scopes, and the verbatim option ID sent back to the agent.
    let cases = [
        (
            "cursor",
            serde_json::json!([
                {"optionId":"allow-once","name":"Allow once","kind":"allow_once"},
                {"optionId":"allow-always","name":"Always allow","kind":"allow_always"},
                {"optionId":"reject-once","name":"Reject","kind":"reject_once"}
            ]),
            serde_json::json!([
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}},
                {"optionId":"allow-always","label":"Always allow","choice":{"effect":"allow","scope":{"persistent":{"where_stored":"the agent's own persistent permissions (location not reported)"}}}},
                {"optionId":"reject-once","label":"Reject","choice":{"effect":"decline","scope":"once"}}
            ]),
        ),
        (
            "claude",
            serde_json::json!([
                {"optionId":"allow-once","name":"Allow once","kind":"allow_once"},
                {"optionId":"allow-with-updates","name":"Allow with updates","kind":"allow_always"},
                {"optionId":"allow-skill-exact","name":"Allow this skill","kind":"allow_always"},
                {"optionId":"reject","name":"Reject","kind":"reject_once"}
            ]),
            serde_json::json!([
                {"optionId":"allow-once","label":"Allow once","choice":{"effect":"allow","scope":"once"}},
                {"optionId":"allow-with-updates","label":"Allow with updates","choice":{"effect":"allow","scope":{"persistent":{"where_stored":"the agent's own persistent permissions (location not reported)"}}}},
                {"optionId":"allow-skill-exact","label":"Allow this skill","choice":{"effect":"allow","scope":{"persistent":{"where_stored":"the agent's own persistent permissions (location not reported)"}}}},
                {"optionId":"reject","label":"Reject","choice":{"effect":"decline","scope":"once"}}
            ]),
        ),
    ];
    for (agent_name, options, expected_options) in cases {
        let reject_id = if agent_name == "cursor" {
            "reject-once"
        } else {
            "reject"
        };
        for (decision, selected_id) in [
            (ApprovalDecision::Allow, "allow-once"),
            (ApprovalDecision::Deny, reject_id),
        ] {
            let root = tempfile::tempdir()?;
            let service_id =
                UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
            let generation = CodexGeneration {
                service_epoch: service_id.clone(),
                generation: GenerationNumber::try_from(1)?,
            };
            let fixture = acp_scripted_fixture::AcpFixtureScript::new()
            .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion":1}))
            .respond("initialize", serde_json::json!({"protocolVersion":1,"agentCapabilities":{},"agentInfo":{"name":agent_name,"version":"1"}}))
            .expect_request("create", "session/new", serde_json::json!({}))
            .respond("create", serde_json::json!({"sessionId":"session-a"}))
            .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId":"session-a"}))
            .send(serde_json::json!({"jsonrpc":"2.0","id":91,"method":"session/request_permission","params":{"sessionId":"session-a","toolCall":{"toolCallId":"tool-a","title":"Run command","kind":"execute"},"options":options.clone()}}))
            .expect_message(serde_json::json!({"jsonrpc":"2.0","id":91,"result":{"outcome":{"outcome":"selected","optionId":selected_id}}}))
            .respond("prompt", serde_json::json!({"stopReason":"end_turn"}))
            .launch();
            let runtime = ExternalProviderRuntime::initialize(fixture).await?;
            let (broker, approval_notice) =
                approval_broker_fixture(&root, &service_id, &generation).await?;
            runtime.install_approval_broker(Arc::clone(&broker)).await;
            runtime.create_session(root.path().to_owned()).await?;
            let approver = session_ref(&service_id, "codex-local", "approver")?;
            let mut prompt = Box::pin(runtime.prompt_with_approval_context(
                "session-a".to_owned(),
                "Run command.".to_owned(),
                ExternalProviderApprovalContext {
                    requester: (approver.clone()).into(),
                    approver: (approver.clone()).into(),
                    target: session_ref(&service_id, "cursor-local", "session-a")?,
                    operation_id: OperationId::generate(),
                    binding_generation: generation,
                    binding_retirement: CancellationToken::new(),
                },
            ));
            wait_for_pending_approval(&broker, &approval_notice, prompt.as_mut()).await?;
            let pending = observed_typed_approvals(&broker, true).await;
            assert_eq!(pending.len(), 1, "{agent_name}");
            assert_eq!(
                serde_json::to_value(&pending[0].request.options)?,
                expected_options,
                "{agent_name}"
            );
            assert_eq!(
                serde_json::to_value(&pending[0].request.subject)?,
                serde_json::json!({"type":"tool_call","toolCall":{"toolCallId":"tool-a","kind":"Execute","title":"Run command"}}),
                "{agent_name}"
            );
            assert!(matches!(
                broker
                    .decide(ApprovalDecideParams {
                        request_id: pending[0].request_id.clone(),
                        decision: Some(ApprovalDecision::AllowForSession),
                        option_id: None,
                        acknowledge_persistent: false,
                        note: None,
                        actor: approval_actor(&approver),
                    })
                    .await,
                Err(error) if error.code() == "decisionNotOffered"
            ));
            assert_eq!(observed_typed_approvals(&broker, true).await.len(), 1);
            broker
                .decide(ApprovalDecideParams {
                    request_id: pending[0].request_id.clone(),
                    decision: Some(decision),
                    option_id: None,
                    acknowledge_persistent: false,
                    note: None,
                    actor: approval_actor(&approver),
                })
                .await?;
            let outcome = tokio::time::timeout(Duration::from_secs(5), &mut prompt).await??;
            assert_eq!(outcome.stop_reason, ProviderPromptStopReason::EndTurn);
            assert_eq!(
                observed_typed_approvals(&broker, false).await[0].state,
                collaboration_protocol::ApprovalState::Decided
            );
            runtime.shutdown().await;
        }
    }
    Ok(())
}

#[tokio::test]
async fn output_limit_cancels_pending_permission() -> TestResult {
    let root = tempfile::tempdir()?;
    let event_socket = root.path().join("output-limit-event.sock");
    let listener = tokio::net::UnixListener::bind(&event_socket)?;
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(
        output_limit_with_pending_permission_fixture(&event_socket),
    )
    .await?;
    let (broker, approval_notice) =
        approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    let requester = session_ref(&service_id, "codex-local", "approver")?;
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Produce output.".to_owned(),
        ExternalProviderApprovalContext {
            requester: (requester.clone()).into(),
            approver: (requester).into(),
            target: session_ref(&service_id, "cursor-local", "session-a")?,
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: CancellationToken::new(),
        },
    ));
    wait_for_pending_approval(&broker, &approval_notice, prompt.as_mut()).await?;
    let (mut event, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await??;
    tokio::io::AsyncWriteExt::write_all(&mut event, b"g").await?;
    let error = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
        .await?
        .expect_err("output limit stops prompt");
    assert!(
        matches!(
            error,
            ExternalProviderRuntimeError::PromptOutputLimitExceeded
        ),
        "unexpected output-limit error: {error:?}"
    );
    let history = observed_typed_approvals(&broker, false).await;
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].state,
        collaboration_protocol::ApprovalState::Cancelled
    );
    assert_eq!(history[0].reason.as_deref(), Some("turnCancelled"));
    assert!(observed_typed_approvals(&broker, true).await.is_empty());
    runtime.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn provider_exit_cancels_pending_permission_as_provider_retired() -> TestResult {
    // ACP v1 prompt-turn.mdx:365-367 ends a Turn only at the agent's prompt
    // result. Specification R5 projects connection loss as providerRetired.
    let root = tempfile::tempdir_in("/tmp")?;
    let exit_socket_path = root.path().join("fixture-exit.sock");
    let exit_listener = tokio::net::UnixListener::bind(&exit_socket_path)?;
    let fixture = acp_scripted_fixture::AcpFixtureScript::new()
        .expect_request("initialize", "initialize", serde_json::json!({"protocolVersion": 1}))
        .respond("initialize", serde_json::json!({"protocolVersion": 1, "agentCapabilities": {}, "agentInfo": {"name": "loss-fixture", "version": "1"}}))
        .expect_request("create", "session/new", serde_json::json!({}))
        .respond("create", serde_json::json!({"sessionId": "session-a"}))
        .expect_request("prompt", "session/prompt", serde_json::json!({"sessionId": "session-a"}))
        .send(serde_json::json!({"jsonrpc": "2.0", "id": 91, "method": "session/request_permission", "params": {"sessionId": "session-a", "toolCall": {"toolCallId": "permission-a", "title": "Run an approved command", "kind": "execute"}, "options": [{"optionId": "allow-a", "name": "Allow once", "kind": "allow_once"}]}}))
        .exit_on_socket_signal(&exit_socket_path)
        .record_diagnostics(root.path().join("fixture-diagnostics.txt"))
        .launch();
    let service_id = UuidIdentity::try_from("0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89".to_owned())?;
    let generation = CodexGeneration {
        service_epoch: service_id.clone(),
        generation: GenerationNumber::try_from(1)?,
    };
    let runtime = ExternalProviderRuntime::initialize(fixture).await?;
    let (broker, approval_notice) =
        approval_broker_fixture(&root, &service_id, &generation).await?;
    runtime.install_approval_broker(Arc::clone(&broker)).await;
    runtime.create_session(root.path().to_owned()).await?;
    let approver = session_ref(&service_id, "codex-local", "approver")?;
    let mut prompt = Box::pin(runtime.prompt_with_approval_context(
        "session-a".to_owned(),
        "Run a command requiring permission.".to_owned(),
        ExternalProviderApprovalContext {
            requester: (approver.clone()).into(),
            approver: approver.into(),
            target: session_ref(&service_id, "cursor-local", "session-a")?,
            operation_id: OperationId::generate(),
            binding_generation: generation,
            binding_retirement: runtime.retirement(),
        },
    ));
    if let Err(error) = wait_for_pending_approval(&broker, &approval_notice, prompt.as_mut()).await
    {
        let diagnostics = std::fs::read_to_string(root.path().join("fixture-diagnostics.txt"))
            .unwrap_or_default();
        return Err(format!("{error}; fixture: {diagnostics}").into());
    }
    let (mut exit_signal, _) = tokio::time::timeout(Duration::from_secs(5), exit_listener.accept())
        .await
        .map_err(|_| "fixture did not arm its exit signal")??;
    exit_signal.write_all(b"x").await?;
    let prompt_result = tokio::time::timeout(Duration::from_secs(5), &mut prompt)
        .await
        .map_err(|_| "prompt did not settle after fixture agent exited")?;
    assert!(
        matches!(
            prompt_result,
            Err(ExternalProviderRuntimeError::TransportFailure)
        ),
        "{prompt_result:?}"
    );
    tokio::time::timeout(Duration::from_secs(5), runtime.retirement().cancelled())
        .await
        .map_err(|_| "provider connection did not retire after fixture exit")?;
    runtime.shutdown().await;
    let history = observed_typed_approvals(&broker, false).await;
    assert_eq!(history.len(), 1);
    assert_eq!(
        history[0].state,
        collaboration_protocol::ApprovalState::Cancelled
    );
    assert_eq!(history[0].reason.as_deref(), Some("providerRetired"));
    assert!(observed_typed_approvals(&broker, true).await.is_empty());
    Ok(())
}
