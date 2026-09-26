use collaboration_protocol::QuestionResponse;
use collaboration_service::{
    HubEvent, NativeControlBackend, NativeGenerationGate, RouterSessionAppServerContext,
    ServiceApprovalBroker, SessionCommandPort, SessionEventHub,
    serve_router_session_app_server_connection,
};
use futures_util::{SinkExt, StreamExt};
use message_board::Identity;
use serde_json::{Value, json};
use session_event_model::{
    ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, ApprovalSubject, OfferedOption,
    OfferedOptionId, OfferedOptions, PendingInteraction, QuestionRequest, SessionEvent,
};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use tokio_tungstenite::{client_async, tungstenite::Message};

#[path = "support/acp_provider_backend.rs"]
mod acp_provider_backend;
use acp_provider_backend::ScriptedProviderBackend;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn app_server_approver_decides_exact_option_and_answers_question() -> TestResult {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let backend = Arc::new(ScriptedProviderBackend::new()?);
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
    let actor: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
    let socket = directory.path().join("claude.sock");
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            actor.clone(),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        )
        .with_interaction_broker(Arc::clone(&broker)),
    );
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        serve_router_session_app_server_connection(stream, context)
            .await
            .map_err(std::io::Error::other)
    });
    let stream = tokio::net::UnixStream::connect(&socket).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{"cwd":"/tmp"}})
                .to_string()
                .into(),
        ))
        .await?;
    let started: Value =
        serde_json::from_str(client.next().await.ok_or("thread start")??.to_text()?)?;
    assert!(
        started["result"]["thread"]["id"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
    );
    let request = ApprovalRequest {
        request_id: "approval-1".into(),
        title: "Run checks".into(),
        description: None,
        subject: Some(ApprovalSubject::Command {
            command: "cargo test".into(),
            cwd: "/tmp".into(),
        }),
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
    };
    let agent_reply = broker
        .request_typed_approval(backend.session.clone(), actor.clone(), request.clone())
        .await?;
    let _sent = backend.events.send(HubEvent {
        sequence: 90,
        event: SessionEvent::TurnStarted {
            turn_id: "turn-1".into(),
            input_id: "input-1".into(),
        },
    });
    let turn_started: Value =
        serde_json::from_str(client.next().await.ok_or("turn start")??.to_text()?)?;
    assert_eq!(turn_started["method"], "turn/started");
    let _sent = backend.events.send(HubEvent {
        sequence: 91,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Approval {
                approver: actor.clone(),
                request: Box::new(request),
            },
        },
    });
    let prompt: Value =
        serde_json::from_str(client.next().await.ok_or("approval prompt")??.to_text()?)?;
    assert_eq!(prompt["method"], "item/commandExecution/requestApproval");
    assert_eq!(prompt["params"]["availableDecisions"][0], "accept");
    client
        .send(Message::Text(
            json!({"id":prompt["id"],"result":{"decision":"accept"}})
                .to_string()
                .into(),
        ))
        .await?;
    let selected = tokio::time::timeout(Duration::from_secs(2), agent_reply).await??;
    assert_eq!(selected.as_str(), "allow-once");

    let question: QuestionRequest = serde_json::from_value(json!({
        "requestId":"question-1","prompt":"Choose count","fields":[
            {"kind":"number","fieldId":"count","label":"Count","description":null,"required":true}
        ]
    }))?;
    let answer = broker
        .request_question(backend.session.clone(), actor.clone(), question.clone())
        .await?;
    let _sent = backend.events.send(HubEvent {
        sequence: 92,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Question {
                approver: actor.clone(),
                request: Box::new(question),
            },
        },
    });
    let form: Value =
        serde_json::from_str(client.next().await.ok_or("question form")??.to_text()?)?;
    assert_eq!(form["method"], "mcpServer/elicitation/request");
    assert_eq!(
        form["params"]["requestedSchema"]["properties"]["count"]["type"],
        "number"
    );
    client
        .send(Message::Text(
            json!({"id":form["id"],"result":{"action":"accept","content":{"count":3}}})
                .to_string()
                .into(),
        ))
        .await?;
    let response = tokio::time::timeout(Duration::from_secs(2), answer).await??;
    assert_eq!(
        response,
        QuestionResponse::Answered {
            content: serde_json::from_value(json!({"count":3}))?
        }
    );
    let other: Identity = serde_json::from_value(json!({"kind":"human","humanId":"other"}))?;
    let other_socket = directory.path().join("other.sock");
    let other_listener = tokio::net::UnixListener::bind(&other_socket)?;
    let other_context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            other,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        )
        .with_interaction_broker(Arc::clone(&broker)),
    );
    let other_server = tokio::spawn(async move {
        let (stream, _) = other_listener.accept().await?;
        serve_router_session_app_server_connection(stream, other_context)
            .await
            .map_err(std::io::Error::other)
    });
    let other_stream = tokio::net::UnixStream::connect(&other_socket).await?;
    let (mut other_client, _) = client_async("ws://localhost/rpc", other_stream).await?;
    other_client
        .send(Message::Text(
            json!({"id":3,"method":"thread/read","params":{
                "threadId":started["result"]["thread"]["id"]
            }})
            .to_string()
            .into(),
        ))
        .await?;
    let other_read: Value = serde_json::from_str(
        other_client
            .next()
            .await
            .ok_or("other thread read")??
            .to_text()?,
    )?;
    assert_eq!(other_read["id"], 3);
    let second_request = ApprovalRequest {
        request_id: "approval-2".into(),
        title: "Run again".into(),
        description: None,
        subject: Some(ApprovalSubject::Command {
            command: "cargo check".into(),
            cwd: "/tmp".into(),
        }),
        options: OfferedOptions::new(vec![OfferedOption {
            option_id: OfferedOptionId::new("allow-again")?,
            label: "Allow once".into(),
            choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
        }])?,
    };
    let second_agent_reply = broker
        .request_typed_approval(
            backend.session.clone(),
            actor.clone(),
            second_request.clone(),
        )
        .await?;
    let _sent = backend.events.send(HubEvent {
        sequence: 93,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Approval {
                approver: actor,
                request: Box::new(second_request),
            },
        },
    });
    let other_notice: Value = serde_json::from_str(
        other_client
            .next()
            .await
            .ok_or("read-only notice")??
            .to_text()?,
    )?;
    assert_eq!(other_notice["method"], "item/started");
    assert_eq!(other_notice["params"]["item"]["type"], "agentMessage");
    let owner_prompt: Value = serde_json::from_str(
        client
            .next()
            .await
            .ok_or("second owner prompt")??
            .to_text()?,
    )?;
    assert_eq!(
        owner_prompt["method"],
        "item/commandExecution/requestApproval"
    );
    client
        .send(Message::Text(
            json!({"id":owner_prompt["id"],"result":{"decision":"accept"}})
                .to_string()
                .into(),
        ))
        .await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), second_agent_reply)
            .await??
            .as_str(),
        "allow-again"
    );
    other_client.close(None).await?;
    other_server.await??;
    client.close(None).await?;
    server.await??;
    Ok(())
}
