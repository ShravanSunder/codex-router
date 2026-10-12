use collaboration_protocol::QuestionResponse;
use collaboration_service::{
    HubEvent, NativeControlBackend, NativeGenerationGate, RouterSessionAppServerContext,
    RouterSessionAppServerListener, ServiceInteractionBroker, SessionCommandPort, SessionEventHub,
};
use futures_util::{SinkExt, StreamExt};
use message_board::Identity;
use serde_json::{Value, json};
use session_event_model::{PendingInteraction, QuestionRequest, SessionEvent};
use std::{os::unix::fs::PermissionsExt, sync::Arc, time::Duration};
use tokio_tungstenite::{client_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;

#[path = "support/acp_provider_backend.rs"]
mod acp_provider_backend;
use acp_provider_backend::ScriptedProviderBackend;

/// A client-level JSON-RPC error means the TUI cannot render a server request.
/// The broker question remains for another front door, with no request loop.
#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn client_error_does_not_retry_or_settle_question() -> Result<(), Box<dyn std::error::Error>>
{
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let backend = Arc::new(ScriptedProviderBackend::new()?);
    let actor: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
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
    let socket = directory.path().join("provider.sock");
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            actor.clone(),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_interaction_broker(Arc::clone(&broker)),
    );
    let listener = RouterSessionAppServerListener::bind(&socket, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
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
    assert_eq!(started["id"], 1);

    let question: QuestionRequest = serde_json::from_value(json!({
        "requestId":"client-error","prompt":"Choose an option","fields":[
            {"kind":"singleChoice","fieldId":"item","label":"Item","description":null,
             "required":true,"options":[
                {"optionId":"first","label":"First"},
                {"optionId":"second","label":"Second"}
             ]}
        ]
    }))?;
    let agent_reply = broker
        .request_question(
            backend.session.clone(),
            actor.clone(),
            question.clone(),
            None,
        )
        .await?;
    let _sent = backend.events.send(HubEvent {
        sequence: 10,
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
    client
        .send(Message::Text(
            json!({"id":form["id"],"error":{"code":-32601,"message":"unsupported schema"}})
                .to_string()
                .into(),
        ))
        .await?;
    client
        .send(Message::Text(
            json!({"id":3,"method":"thread/list","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    let next: Value = serde_json::from_str(
        tokio::time::timeout(Duration::from_secs(2), client.next())
            .await?
            .ok_or("ordering response")??
            .to_text()?,
    )?;
    assert_eq!(next["id"], 3, "client rejection reissued a request: {next}");
    assert!(
        broker
            .list_questions(true)
            .await
            .iter()
            .any(|record| record.request_id() == "client-error")
    );
    broker
        .respond_question(
            "client-error",
            &actor,
            QuestionResponse::Answered {
                content: serde_json::from_value(json!({
                    "item":{"selectedOptionIds":["first"]}
                }))?,
            },
        )
        .await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), agent_reply).await??,
        QuestionResponse::Answered { .. }
    ));
    client.close(None).await?;
    shutdown.cancel();
    running.await??;
    Ok(())
}

/// A decision already made by another front door does not become a terminal
/// TUI error while the provider Turn is still settling.
#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn stale_question_answer_does_not_emit_nonretry_error()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_id = "0ff962c5-7fa3-4c18-a5ca-1bbe8db09e89";
    let backend = Arc::new(ScriptedProviderBackend::new()?);
    let actor: Identity = serde_json::from_value(json!({"kind":"human","humanId":"owner"}))?;
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
    let socket = directory.path().join("provider.sock");
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            actor.clone(),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_interaction_broker(Arc::clone(&broker)),
    );
    let listener = RouterSessionAppServerListener::bind(&socket, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    let stream = tokio::net::UnixStream::connect(&socket).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{"cwd":"/tmp"}})
                .to_string()
                .into(),
        ))
        .await?;
    assert_eq!(
        serde_json::from_str::<Value>(client.next().await.ok_or("thread start")??.to_text()?)?["id"],
        1
    );
    let question: QuestionRequest = serde_json::from_value(json!({
        "requestId":"stale-answer","prompt":"Ready?","fields":[
            {"kind":"boolean","fieldId":"ready","label":"Ready","description":null,"required":true}
        ]
    }))?;
    let agent_reply = broker
        .request_question(
            backend.session.clone(),
            actor.clone(),
            question.clone(),
            None,
        )
        .await?;
    let _sent = backend.events.send(HubEvent {
        sequence: 10,
        event: SessionEvent::InteractionRequested {
            interaction: PendingInteraction::Question {
                approver: actor.clone(),
                request: Box::new(question),
            },
        },
    });
    let form: Value =
        serde_json::from_str(client.next().await.ok_or("question form")??.to_text()?)?;
    broker
        .respond_question(
            "stale-answer",
            &actor,
            QuestionResponse::Answered {
                content: serde_json::from_value(json!({"ready":true}))?,
            },
        )
        .await?;
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), agent_reply).await??,
        QuestionResponse::Answered { .. }
    ));
    client
        .send(Message::Text(
            json!({"id":form["id"],"result":{"action":"accept","content":{"ready":false}}})
                .to_string()
                .into(),
        ))
        .await?;
    client
        .send(Message::Text(
            json!({"id":3,"method":"thread/list","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    let next: Value = serde_json::from_str(
        tokio::time::timeout(Duration::from_secs(2), client.next())
            .await?
            .ok_or("ordering response")??
            .to_text()?,
    )?;
    assert_eq!(
        next["id"], 3,
        "stale decision finalized the TUI Turn: {next}"
    );
    client.close(None).await?;
    shutdown.cancel();
    running.await??;
    Ok(())
}
