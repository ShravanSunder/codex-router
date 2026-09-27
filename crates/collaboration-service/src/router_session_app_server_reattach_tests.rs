use super::test_support::ScriptedSessionBackend;
use super::*;
use session_event_model::SessionEvent;
use std::{os::unix::fs::PermissionsExt, sync::Arc};
use tokio::net::UnixListener;
use tokio_tungstenite::client_async;

/// A resolved historical approval must not become a fresh server request on
/// reattach. The next client request is an ordering barrier for attach replay.
#[tokio::test]
async fn reattach_does_not_reissue_resolved_approval() -> Result<(), Box<dyn std::error::Error>> {
    use crate::{NativeControlBackend, NativeGenerationGate, ServiceInteractionBroker};
    use session_event_model::{
        ApprovalChoice, ApprovalEffect, ApprovalRequest, ApprovalScope, OfferedOption,
        OfferedOptionId, OfferedOptions, OptionsOrigin, PendingInteraction, StopReason,
        TurnOutcome,
    };
    use tokio::time::{Duration, timeout};

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let backend = ScriptedSessionBackend::new()?;
    let actor = ScriptedSessionBackend::actor()?;
    let native_endpoint = serde_json::from_value(json!({
        "serviceId":backend.endpoint.service_id,
        "endpointId":"claude-local"
    }))?;
    let broker = ServiceInteractionBroker::load(
        backend.endpoint.service_id.as_str().to_owned().try_into()?,
        NativeControlBackend {
            endpoint: native_endpoint,
            gate: NativeGenerationGate::default(),
            codex_home: directory.path().to_path_buf(),
        },
        directory.path().join("approvals.json"),
    )
    .await?;
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            actor.clone(),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            tokio::sync::watch::channel(Vec::new()).1,
        )
        .with_interaction_broker(broker),
    );
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (stream, _) = listener.accept().await.expect("accept client");
            serve_router_session_app_server_connection(stream, Arc::clone(&context))
                .await
                .expect("serve client");
        }
    });

    let stream = UnixStream::connect(&socket_path).await?;
    let (mut first, _) = client_async("ws://localhost/rpc", stream).await?;
    first
        .send(Message::Text(
            json!({"id":1,"method":"thread/start","params":{"cwd":directory.path()}})
                .to_string()
                .into(),
        ))
        .await?;
    let started: Value =
        serde_json::from_str(first.next().await.ok_or("thread/start")??.to_text()?)?;
    let thread_id = started["result"]["thread"]["id"]
        .as_str()
        .ok_or("thread id")?
        .to_owned();
    first.close(None).await?;

    let approval = ApprovalRequest {
        request_id: "already-resolved".into(),
        title: "Run scratch command".into(),
        description: None,
        subject: Some(session_event_model::ApprovalSubject::Command {
            command: "pwd".into(),
            cwd: directory.path().display().to_string(),
        }),
        options_origin: OptionsOrigin::AgentOffered,
        options: OfferedOptions::new(vec![OfferedOption {
            option_id: OfferedOptionId::new("allow-once")?,
            label: "Allow once".into(),
            choice: ApprovalChoice::new(ApprovalEffect::Allow, ApprovalScope::Once),
        }])?,
    };
    *backend.history.lock().expect("test lock") = vec![
        crate::HubEvent {
            sequence: 1,
            event: SessionEvent::TurnStarted {
                turn_id: "historical-turn".into(),
                input_id: session_event_model::InputId::generate(),
            },
        },
        crate::HubEvent {
            sequence: 2,
            event: SessionEvent::InteractionRequested {
                interaction: PendingInteraction::Approval {
                    approver: actor,
                    request: Box::new(approval),
                },
            },
        },
        crate::HubEvent {
            sequence: 3,
            event: SessionEvent::InteractionResolved {
                request_id: "already-resolved".into(),
            },
        },
        crate::HubEvent {
            sequence: 4,
            event: SessionEvent::TurnEnded {
                turn_id: "historical-turn".into(),
                outcome: TurnOutcome::Ended {
                    stop_reason: StopReason::EndTurn,
                    local_cause: None,
                },
            },
        },
    ];
    let stream = UnixStream::connect(&socket_path).await?;
    let (mut second, _) = client_async("ws://localhost/rpc", stream).await?;
    second
        .send(Message::Text(
            json!({"id":2,"method":"thread/resume","params":{"threadId":thread_id}})
                .to_string()
                .into(),
        ))
        .await?;
    let resumed: Value =
        serde_json::from_str(second.next().await.ok_or("thread/resume")??.to_text()?)?;
    assert_eq!(resumed["id"], 2);
    second
        .send(Message::Text(
            json!({"id":3,"method":"thread/list","params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    let next: Value = serde_json::from_str(
        timeout(Duration::from_secs(2), second.next())
            .await?
            .ok_or("thread/list")??
            .to_text()?,
    )?;
    assert_eq!(
        next["id"], 3,
        "resolved approval was replayed as a live request: {next}"
    );
    second.close(None).await?;
    server.await?;
    Ok(())
}
