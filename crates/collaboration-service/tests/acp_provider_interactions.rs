use codex_acp_adapter::{AcpSchemaCatalog, NativeStoredSessions};
use collaboration_protocol::QuestionResponse;
use collaboration_service::{
    AcpChannelListener, HubEvent, NativeControlBackend, NativeGenerationGate,
    ServiceApprovalBroker, SessionCommandPort, SessionEventHub, UnmaterializedThreadHolder,
};
use message_board::{Identity, SessionEndpointRef};
use serde_json::{Value, json};
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, OfferedOption, OfferedOptionId,
    OfferedOptions, PendingInteraction, PendingInteractions, QuestionRequest, SessionEvent,
    SessionState,
};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[path = "support/acp_provider_backend.rs"]
mod acp_provider_backend;
use acp_provider_backend::ScriptedProviderBackend;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn next_frame(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Result<Value, Box<dyn std::error::Error>> {
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await??
        .ok_or("ACP connection closed")?;
    Ok(serde_json::from_str(&line)?)
}

async fn send_frame(writer: &mut tokio::net::unix::OwnedWriteHalf, frame: Value) -> TestResult {
    writer.write_all(format!("{frame}\n").as_bytes()).await?;
    Ok(())
}

#[allow(clippy::panic_in_result_fn)]
async fn connect_actor(
    path: &Path,
    human_id: &str,
) -> Result<
    (
        tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
        tokio::net::unix::OwnedWriteHalf,
    ),
    Box<dyn std::error::Error>,
> {
    let stream = tokio::net::UnixStream::connect(path).await?;
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":1,"clientCapabilities":{"elicitation":{"form":{}}},
            "_meta":{"router":{"actor":{"kind":"human","humanId":human_id}},
                "sessionProfile":{"version":1,"elements":["state"]}}
        }}),
    )
    .await?;
    assert_eq!(
        next_frame(&mut lines)
            .await?
            .pointer("/result/protocolVersion"),
        Some(&json!(1))
    );
    Ok((lines, writer))
}

fn approval(request_id: &str) -> Result<ApprovalRequest, Box<dyn std::error::Error>> {
    Ok(ApprovalRequest {
        request_id: request_id.into(),
        title: "Run checks".into(),
        description: Some("Approve execution".into()),
        subject: None,
        options: OfferedOptions::new(vec![
            OfferedOption {
                option_id: OfferedOptionId::new("allow-once")?,
                label: "Allow once".into(),
                choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
            },
            OfferedOption {
                option_id: OfferedOptionId::new("reject-once")?,
                label: "Reject once".into(),
                choice: ApprovalChoice::new(ApprovalEffect::Decline, ApprovalScope::Once),
            },
        ])?,
    })
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn only_the_approver_receives_and_decides_exact_provider_options() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let provider = Arc::new(ScriptedProviderBackend::new()?);
    let endpoint: SessionEndpointRef = provider.endpoint.clone();
    let broker = ServiceApprovalBroker::load(
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
        endpoint,
        Arc::clone(&provider) as Arc<dyn SessionCommandPort>,
        Arc::clone(&provider) as Arc<dyn SessionEventHub>,
    )
    .with_interaction_broker(Arc::clone(&broker));
    let stop = CancellationToken::new();
    let serving = tokio::spawn(listener.run(stop.clone()));
    let owner: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
    let (mut owner_lines, mut owner_writer) = connect_actor(&socket, "owner").await?;
    send_frame(
        &mut owner_writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}
        }}),
    )
    .await?;
    assert_eq!(
        next_frame(&mut owner_lines).await?["result"]["sessionId"],
        provider.session.session_id.as_str()
    );
    let (mut other_lines, mut other_writer) = connect_actor(&socket, "other").await?;
    send_frame(
        &mut other_writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{
            "sessionId":provider.session.session_id.as_str(),"cwd":"/tmp","mcpServers":[],
            "_meta":{"router":{"sessionRef":provider.session}}
        }}),
    )
    .await?;
    assert_eq!(next_frame(&mut other_lines).await?["id"], 2);

    let request = approval("approval-1")?;
    let agent_reply = broker
        .request_typed_approval(provider.session.clone(), owner.clone(), request.clone())
        .await?;
    let pending = PendingInteraction::Approval {
        approver: owner.clone(),
        request: Box::new(request),
    };
    let _sent = provider.events.send(HubEvent {
        sequence: 70,
        event: SessionEvent::InteractionRequested {
            interaction: pending.clone(),
        },
    });
    let _sent = provider.events.send(HubEvent {
        sequence: 71,
        event: SessionEvent::StateChanged {
            state: SessionState::RequiresAction {
                pending: PendingInteractions::new(vec![pending]).ok_or("pending")?,
            },
        },
    });
    let first = next_frame(&mut owner_lines).await?;
    let second = next_frame(&mut owner_lines).await?;
    let offered = if first["method"] == "session/request_permission" {
        first
    } else {
        second
    };
    assert_eq!(offered["method"], "session/request_permission");
    assert!(AcpSchemaCatalog::load()?.validate("RequestPermissionRequest", &offered["params"])?);
    assert_eq!(offered["params"]["options"][0]["optionId"], "allow-once");
    assert_eq!(offered["params"]["options"][1]["optionId"], "reject-once");
    let other_state = next_frame(&mut other_lines).await?;
    assert_eq!(other_state["method"], "_session/state");
    assert_eq!(other_state["params"]["state"], "requires_action");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), other_lines.next_line())
            .await
            .is_err()
    );
    send_frame(
        &mut owner_writer,
        json!({"jsonrpc":"2.0","id":offered["id"],"result":{
            "outcome":{"outcome":"selected","optionId":"allow-once"}
        }}),
    )
    .await?;
    let selected = tokio::time::timeout(Duration::from_secs(2), agent_reply).await??;
    assert_eq!(selected.as_str(), "allow-once");
    stop.cancel();
    serving.await??;
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn approver_question_form_returns_typed_answers_to_the_agent() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let provider = Arc::new(ScriptedProviderBackend::new()?);
    let broker = ServiceApprovalBroker::load(
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
    let owner: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
    let (mut lines, mut writer) = connect_actor(&socket, "owner").await?;
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/tmp","mcpServers":[],"_meta":{"router":{"endpoint":"claude-local"}}
        }}),
    )
    .await?;
    assert_eq!(next_frame(&mut lines).await?["id"], 2);
    let request: QuestionRequest = serde_json::from_value(json!({
        "requestId":"question-1","prompt":"Choose run settings","fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true},
            {"kind":"boolean","fieldId":"dryRun","label":"Dry run","description":null,"required":true}
        ]
    }))?;
    let agent_reply = broker
        .request_question(provider.session.clone(), owner.clone(), request.clone())
        .await?;
    let _sent = provider.events.send(HubEvent {
        sequence: 80,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Question {
                approver: owner,
                request: Box::new(request),
            },
        },
    });
    let elicitation = next_frame(&mut lines).await?;
    assert_eq!(elicitation["method"], "elicitation/create");
    assert!(
        AcpSchemaCatalog::load()?.validate("CreateElicitationRequest", &elicitation["params"])?
    );
    assert_eq!(
        elicitation["params"]["requestedSchema"]["properties"]["count"]["type"],
        "number"
    );
    assert_eq!(
        elicitation["params"]["requestedSchema"]["properties"]["dryRun"]["type"],
        "boolean"
    );
    send_frame(
        &mut writer,
        json!({"jsonrpc":"2.0","id":elicitation["id"],"result":{
            "action":"accept","content":{"count":3,"dryRun":true}
        }}),
    )
    .await?;
    let response = tokio::time::timeout(Duration::from_secs(2), agent_reply).await??;
    assert_eq!(
        response,
        QuestionResponse::Answered {
            content: serde_json::from_value(json!({"count":3,"dryRun":true}))?
        }
    );
    stop.cancel();
    serving.await??;
    Ok(())
}
