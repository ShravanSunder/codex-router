use super::*;

/// A resolved historical request may remain in the hub snapshot, but a new
/// ACP connection must not present it as a live permission request.
#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn acp_attach_does_not_represent_resolved_historical_approval() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let provider = Arc::new(ScriptedProviderBackend::new()?);
    let owner: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
    *provider.snapshot_events.lock().expect("snapshot lock") = vec![
        HubEvent {
            sequence: 3,
            event: SessionEvent::InteractionRequested {
                interaction: PendingInteraction::Approval {
                    approver: owner,
                    request: Box::new(approval("historical-resolved")?),
                },
            },
        },
        HubEvent {
            sequence: 4,
            event: SessionEvent::InteractionResolved {
                request_id: "historical-resolved".into(),
            },
        },
    ];
    let broker = ServiceInteractionBroker::load(
        service_id.to_owned().try_into()?,
        NativeControlBackend {
            endpoint: serde_json::from_value(
                json!({"serviceId":service_id,"endpointId":"claude-local"}),
            )?,
            gate: NativeGenerationGate::default(),
            codex_home: directory.path().to_path_buf(),
        },
        directory.path().join("approval-routes.json"),
    )
    .await?;
    let socket = directory.path().join("codex-acp.sock");
    let listener = AcpChannelListener::bind(
        &socket,
        NativeGenerationGate::default(),
        Arc::new(NativeStoredSessions::new(
            directory.path().to_path_buf(),
            "fixture".into(),
        )),
        Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        Arc::new(UnmaterializedThreadHolder::new()),
        Arc::new(collaboration_service::UnavailableConversationOperationRecorder),
    )?
    .with_provider_session_backend(
        provider.endpoint.clone(),
        Arc::clone(&provider) as Arc<dyn SessionCommandPort>,
        Arc::clone(&provider) as Arc<dyn SessionEventHub>,
    )
    .with_interaction_broker(broker);
    let stop = CancellationToken::new();
    let serving = tokio::spawn(listener.run(stop.clone()));
    let (mut lines, mut writer) = connect_actor(&socket, "owner").await?;
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}
        }}),
    )
    .await?;
    assert_eq!(next_frame(&mut lines).await?["id"], 2);
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":3,"method":"session/load","params":{
            "sessionId":provider.session.session_id.as_str(),"cwd":"/tmp","mcpServers":[],
            "_meta":{"router":{"sessionRef":provider.session}}
        }}),
    )
    .await?;
    let next = next_frame(&mut lines).await?;
    assert_eq!(
        next["id"], 3,
        "resolved historical approval replayed: {next}"
    );
    stop.cancel();
    serving.await??;
    Ok(())
}

/// The ACP client can choose a persistent ID without sending the Router
/// acknowledgement. That rejection keeps the approval pending and re-presents
/// the exact offered options with a fresh request ID.
#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn rejected_persistent_acp_choice_is_visible_and_retryable() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let provider = Arc::new(ScriptedProviderBackend::new()?);
    let owner: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
    let broker = ServiceInteractionBroker::load(
        service_id.to_owned().try_into()?,
        NativeControlBackend {
            endpoint: serde_json::from_value(
                json!({"serviceId":service_id,"endpointId":"claude-local"}),
            )?,
            gate: NativeGenerationGate::default(),
            codex_home: directory.path().to_path_buf(),
        },
        directory.path().join("approval-routes.json"),
    )
    .await?;
    let socket = directory.path().join("codex-acp.sock");
    let listener = AcpChannelListener::bind(
        &socket,
        NativeGenerationGate::default(),
        Arc::new(NativeStoredSessions::new(
            directory.path().to_path_buf(),
            "fixture".into(),
        )),
        Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        Arc::new(UnmaterializedThreadHolder::new()),
        Arc::new(collaboration_service::UnavailableConversationOperationRecorder),
    )?
    .with_provider_session_backend(
        provider.endpoint.clone(),
        Arc::clone(&provider) as Arc<dyn SessionCommandPort>,
        Arc::clone(&provider) as Arc<dyn SessionEventHub>,
    )
    .with_interaction_broker(Arc::clone(&broker));
    let stop = CancellationToken::new();
    let serving = tokio::spawn(listener.run(stop.clone()));
    let (mut lines, mut writer) = connect_actor(&socket, "owner").await?;
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}
        }}),
    )
    .await?;
    assert_eq!(next_frame(&mut lines).await?["id"], 2);
    let request = ApprovalRequest {
        request_id: "persistent-retry".into(),
        title: "Allow a command".into(),
        description: None,
        subject: None,
        options_origin: session_event_model::OptionsOrigin::AgentOffered,
        options: OfferedOptions::new(vec![
            OfferedOption {
                option_id: OfferedOptionId::new("allow-always")?,
                label: "Allow always".into(),
                choice: ApprovalChoice::new(
                    ApprovalEffect::Allow,
                    ApprovalScope::persistent("Cursor allowlist")?,
                ),
            },
            OfferedOption {
                option_id: OfferedOptionId::new("reject-once")?,
                label: "Reject once".into(),
                choice: ApprovalChoice::new(ApprovalEffect::Decline, ApprovalScope::Once),
            },
        ])?,
    };
    let agent_reply = broker
        .request_typed_approval(
            provider.session.clone(),
            owner.clone(),
            request.clone(),
            tokio_util::sync::CancellationToken::new(),
            tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await?;
    let _sent = provider.events.send(HubEvent {
        sequence: 10,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Approval {
                approver: owner,
                request: Box::new(request),
            },
        },
    });
    let first = next_frame(&mut lines).await?;
    assert_eq!(first["method"], "session/request_permission");
    assert!(
        first["params"]["options"][0]["name"]
            .as_str()
            .is_some_and(|name| name.contains("requires acknowledgement"))
    );
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":first["id"],"result":{
            "outcome":{"outcome":"selected","optionId":"allow-always"}
        }}),
    )
    .await?;
    let retried = next_frame(&mut lines).await?;
    assert_eq!(retried["method"], "session/request_permission");
    assert_ne!(retried["id"], first["id"]);
    assert_eq!(retried["params"]["options"][0]["optionId"], "allow-always");
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":retried["id"],"result":{
            "outcome":{"outcome":"selected","optionId":"reject-once"}
        }}),
    )
    .await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), agent_reply).await??,
        TypedApprovalResolution::Selected(selection) if selection.option_id.as_str() == "reject-once"
    ));
    stop.cancel();
    serving.await??;
    Ok(())
}

/// Busy is decided from the hub before the command port can submit an input.
#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn busy_acp_prompt_does_not_reach_provider_command_port() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let provider = Arc::new(ScriptedProviderBackend::new()?);
    let socket = directory.path().join("codex-acp.sock");
    let listener = AcpChannelListener::bind(
        &socket,
        NativeGenerationGate::default(),
        Arc::new(NativeStoredSessions::new(
            directory.path().to_path_buf(),
            "fixture".into(),
        )),
        Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        Arc::new(UnmaterializedThreadHolder::new()),
        Arc::new(collaboration_service::UnavailableConversationOperationRecorder),
    )?
    .with_provider_session_backend(
        provider.endpoint.clone(),
        Arc::clone(&provider) as Arc<dyn SessionCommandPort>,
        Arc::clone(&provider) as Arc<dyn SessionEventHub>,
    );
    let stop = CancellationToken::new();
    let serving = tokio::spawn(listener.run(stop.clone()));
    let (mut lines, mut writer) = connect_actor(&socket, "owner").await?;
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}
        }}),
    )
    .await?;
    assert_eq!(next_frame(&mut lines).await?["id"], 2);
    *provider.state.lock().expect("state lock") = SessionState::Running;
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{
            "sessionId":provider.session.session_id.as_str(),
            "prompt":[{"type":"text","text":"must not submit"}]
        }}),
    )
    .await?;
    let rejected = next_frame(&mut lines).await?;
    assert_eq!(rejected["id"], 3);
    assert_eq!(rejected["error"]["code"], -32000);
    assert!(provider.prompted_by.lock().expect("prompt lock").is_empty());
    stop.cancel();
    serving.await??;
    Ok(())
}
