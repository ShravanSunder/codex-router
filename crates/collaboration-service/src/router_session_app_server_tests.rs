use super::test_support::ScriptedSessionBackend;
use super::*;
use session_event_model::{SessionEvent, SessionItem, SessionItemKind, SessionState};
use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};
use tokio::net::UnixListener;
use tokio_tungstenite::client_async;
use tokio_util::sync::CancellationToken;

async fn scripted_exchange(
    socket_path: &Path,
    method: &str,
    expected_result: Option<Value>,
) -> Result<(), Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":7,"method":method,"params":{}})
                .to_string()
                .into(),
        ))
        .await?;
    let reply = client.next().await.ok_or("missing reply")??;
    let value: Value = serde_json::from_str(reply.to_text()?)?;
    assert!(value.get("jsonrpc").is_none());
    assert_eq!(value["id"], 7);
    match expected_result {
        Some(result) => assert_eq!(value["result"], result),
        None => assert_eq!(value["error"]["code"], -32601),
    }
    client.close(None).await?;
    Ok(())
}

/// Oracle: Codex TUI remote requirements sections 0 and 1b, grounded in
/// app-server-client/src/remote.rs and app/startup.rs at e72da2b538.
#[tokio::test]
async fn startup_methods_have_the_exact_remote_wire_shapes()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let socket_path = directory.path().join("claude.sock");
    let listener = UnixListener::bind(&socket_path)?;
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(RouterSessionAppServerContext::new(
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
    ));
    let server = tokio::spawn(async move {
        for _ in 0..9 {
            let (stream, _) = listener.accept().await.expect("accept client");
            serve_router_session_app_server_connection(stream, Arc::clone(&context))
                .await
                .expect("serve request");
        }
    });
    scripted_exchange(&socket_path, "initialize", Some(json!({}))).await?;
    scripted_exchange(
        &socket_path,
        "account/read",
        Some(json!({"account":null,"requiresOpenaiAuth":false})),
    )
    .await?;
    scripted_exchange(
        &socket_path,
        "configRequirements/read",
        Some(json!({"requirements":null})),
    )
    .await?;
    scripted_exchange(&socket_path, "config/read", None).await?;
    scripted_exchange(
        &socket_path,
        "skills/list",
        Some(json!({"data":[],"nextCursor":null})),
    )
    .await?;
    scripted_exchange(
        &socket_path,
        "hooks/list",
        Some(json!({"data":[],"nextCursor":null})),
    )
    .await?;
    scripted_exchange(
        &socket_path,
        "collaborationMode/list",
        Some(json!({"data":[],"nextCursor":null})),
    )
    .await?;
    scripted_exchange(
        &socket_path,
        "thread/list",
        Some(json!({"data":[],"nextCursor":null,"backwardsCursor":null})),
    )
    .await?;

    let stream = UnixStream::connect(&socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":"model","method":"model/list","params":{"includeHidden":true}})
                .to_string()
                .into(),
        ))
        .await?;
    let reply = client.next().await.ok_or("missing model reply")??;
    let value: Value = serde_json::from_str(reply.to_text()?)?;
    assert_eq!(value["id"], "model");
    assert_eq!(
        value["result"],
        json!({"data":[{
        "id":"provider-default","model":"provider-default",
        "upgrade":null,"upgradeInfo":null,"availabilityNux":null,
        "displayName":"provider default",
        "description":"The provider selects its default model",
        "hidden":false,"supportedReasoningEfforts":[],
        "defaultReasoningEffort":"medium","multiAgentVersion":null,
        "isDefault":true
    }],"nextCursor":null})
    );
    client.close(None).await?;
    server.await?;
    Ok(())
}

/// Oracle: Codex TUI remote requirements sections 4 and 1b. Thread IDs
/// must be UUIDs, and the required Thread fields must be returned on start.
#[tokio::test]
async fn thread_start_list_read_resume_use_port_and_hub() -> Result<(), Box<dyn std::error::Error>>
{
    let backend = ScriptedSessionBackend::new()?;
    let request = json!({"cwd":"/tmp","model":"provider-default"});
    let started = handle_app_server_thread_request(
        "thread/start",
        request,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    assert_eq!(started["thread"]["id"], backend.session.session_id.as_str());
    assert_eq!(started["thread"]["status"]["type"], "idle");
    assert_eq!(started["thread"]["cwd"], "/tmp");
    assert_eq!(started["thread"]["turns"], json!([]));
    assert_eq!(started["model"], "provider-default");
    assert_eq!(backend.create_commands.lock().expect("test lock").len(), 1);
    assert!(
        backend.create_commands.lock().expect("test lock")[0]
            .settings
            .model
            .is_none()
    );

    let listed = handle_app_server_thread_request(
        "thread/list",
        json!({}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    assert_eq!(listed["data"][0]["id"], backend.session.session_id.as_str());

    for method in ["thread/read", "thread/resume"] {
        let result = handle_app_server_thread_request(
            method,
            json!({"threadId":backend.session.session_id.as_str()}),
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
        )
        .await?;
        assert_eq!(result["thread"]["id"], backend.session.session_id.as_str());
    }
    Ok(())
}

#[tokio::test]
async fn durable_unloaded_session_has_a_stable_uuid_alias() -> Result<(), Box<dyn std::error::Error>>
{
    let backend = ScriptedSessionBackend::new()?;
    let stored: SessionRef = serde_json::from_value(json!({
        "endpoint":backend.endpoint,
        "sessionId":"provider-conversation-1"
    }))?;
    backend
        .durable_inventory
        .lock()
        .expect("test lock")
        .push(HubSessionSummary {
            session: stored.clone(),
            approver: backend.approver.clone(),
            working_directory: PathBuf::from("/tmp"),
            updated_at_seconds: 1_700_000_000,
            preview: "saved input".into(),
            name: None,
            model: None,
            state: SessionState::Running,
        });
    let listed = handle_app_server_thread_request(
        "thread/list",
        json!({}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    let alias = listed["data"][0]["id"].as_str().ok_or("missing alias")?;
    assert!(Uuid::parse_str(alias).is_ok());
    assert_eq!(alias, thread_alias(&stored));
    assert_eq!(listed["data"][0]["status"]["type"], "notLoaded");
    let read = handle_app_server_thread_request(
        "thread/read",
        json!({"threadId":alias}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    assert_eq!(read["thread"]["id"], alias);
    Ok(())
}

#[tokio::test]
async fn thread_resume_loads_an_unloaded_provider_session() -> Result<(), Box<dyn std::error::Error>>
{
    let backend = ScriptedSessionBackend::new()?;
    backend
        .durable_inventory
        .lock()
        .expect("test lock")
        .push(HubSessionSummary {
            session: backend.session.clone(),
            approver: backend.approver.clone(),
            working_directory: PathBuf::from("/tmp"),
            updated_at_seconds: 1_700_000_000,
            preview: String::new(),
            name: None,
            model: None,
            state: SessionState::Unloaded,
        });
    let actor = ScriptedSessionBackend::actor()?;
    let resumed = handle_app_server_thread_request(
        "thread/resume",
        json!({"threadId":backend.session.session_id.as_str()}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        actor.clone(),
    )
    .await?;
    assert_eq!(resumed["thread"]["status"]["type"], "idle");
    assert_eq!(
        backend.load_commands.lock().expect("test lock").as_slice(),
        &[SessionTargetCommand {
            target: backend.session.clone(),
            actor
        }]
    );
    Ok(())
}

#[tokio::test]
async fn thread_resume_returns_replayed_items_grouped_into_historical_turns()
-> Result<(), Box<dyn std::error::Error>> {
    let backend = ScriptedSessionBackend::new()?;
    backend
        .durable_inventory
        .lock()
        .expect("test lock")
        .push(HubSessionSummary {
            session: backend.session.clone(),
            approver: backend.approver.clone(),
            working_directory: PathBuf::from("/tmp"),
            updated_at_seconds: 1_700_000_000,
            preview: String::new(),
            name: None,
            model: None,
            state: SessionState::Unloaded,
        });
    *backend.history.lock().expect("test lock") = vec![
        HubEvent {
            sequence: 1,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "user-1".into(),
                    kind: SessionItemKind::UserMessage,
                    text: Some("Earlier prompt".into()),
                },
            },
        },
        HubEvent {
            sequence: 2,
            event: SessionEvent::ItemStarted {
                item: SessionItem {
                    item_id: "reply-1".into(),
                    kind: SessionItemKind::AgentMessage,
                    text: Some("Earlier reply".into()),
                },
            },
        },
    ];
    let resumed = handle_app_server_thread_request(
        "thread/resume",
        json!({"threadId":backend.session.session_id.as_str()}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    assert_eq!(
        resumed["thread"]["turns"][0]["items"][0]["type"],
        "userMessage"
    );
    assert_eq!(
        resumed["thread"]["turns"][0]["items"][1]["text"],
        "Earlier reply"
    );
    let read = handle_app_server_thread_request(
        "thread/read",
        json!({"threadId":backend.session.session_id.as_str()}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await?;
    assert_eq!(read["thread"]["turns"], resumed["thread"]["turns"]);
    Ok(())
}

#[tokio::test]
async fn ephemeral_start_never_reaches_the_provider() -> Result<(), Box<dyn std::error::Error>> {
    let backend = ScriptedSessionBackend::new()?;
    let result = handle_app_server_thread_request(
        "thread/start",
        json!({"ephemeral":true,"threadSource":{"feature":"thread_title"}}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
    )
    .await;
    assert!(matches!(result, Err(ThreadMethodError::InvalidParams)));
    assert!(
        backend
            .create_commands
            .lock()
            .expect("test lock")
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn turn_methods_forward_input_and_actor_to_the_port() -> Result<(), Box<dyn std::error::Error>>
{
    let backend = ScriptedSessionBackend::new()?;
    let actor = ScriptedSessionBackend::actor()?;
    let started = handle_app_server_thread_request(
        "thread/start",
        json!({"cwd":"/tmp","model":"provider-default"}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        actor.clone(),
    )
    .await?;
    let thread_id = started["thread"]["id"].as_str().ok_or("thread id")?;
    let turn = handle_app_server_turn_request(
        "turn/start",
        json!({"threadId":thread_id,"input":[{"type":"text","text":"hello"}]}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        actor.clone(),
    )
    .await?;
    assert_eq!(turn["turn"]["id"], "turn-1");
    assert_eq!(turn["turn"]["status"], "inProgress");
    assert_eq!(
        backend.prompt_commands.lock().expect("test lock")[0].actor,
        actor
    );
    assert_eq!(
        backend.prompt_commands.lock().expect("test lock")[0].content,
        vec![crate::CommandContent::text("hello".into())?]
    );

    let steered = handle_app_server_turn_request(
        "turn/steer",
        json!({"threadId":thread_id,"expectedTurnId":"turn-1",
                "input":[{"type":"text","text":"more"}]}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        actor.clone(),
    )
    .await?;
    assert_eq!(steered, json!({"turnId":"turn-1"}));
    assert_eq!(
        backend.steer_commands.lock().expect("test lock")[0].actor,
        actor
    );

    let interrupted = handle_app_server_turn_request(
        "turn/interrupt",
        json!({"threadId":thread_id,"turnId":"turn-1"}),
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        backend.endpoint.clone(),
        actor.clone(),
    )
    .await?;
    assert_eq!(interrupted, json!({}));
    assert_eq!(
        backend.cancel_commands.lock().expect("test lock")[0].actor,
        actor
    );
    Ok(())
}

#[tokio::test]
async fn listener_serves_a_private_provider_socket() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(RouterSessionAppServerContext::new(
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
    ));
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    scripted_exchange(&socket_path, "initialize", Some(json!({}))).await?;
    shutdown.cancel();
    running.await??;
    assert!(!socket_path.exists());
    Ok(())
}

/// The real Codex TUI version is intentionally pinned because its app-server
/// v2 response parser has no protocol version negotiation.
#[tokio::test]
#[ignore = "requires installed codex-cli 0.157.1 and a pseudo-terminal"]
async fn pinned_codex_tui_boots_and_starts_a_provider_thread()
-> Result<(), Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::{
        io::AsyncWriteExt,
        process::Command,
        time::{Duration, timeout},
    };

    let version = Command::new("codex").arg("--version").output().await?;
    assert!(version.status.success());
    assert_eq!(
        String::from_utf8(version.stdout)?.trim(),
        "codex-cli 0.157.1"
    );

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(RouterSessionAppServerContext::new(
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
    ));
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    let mut tui = Command::new("script")
        .arg("-q")
        .arg("/dev/null")
        .arg("codex")
        .arg("--remote")
        .arg(format!("unix://{}", socket_path.display()))
        .arg("--no-alt-screen")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let boot_result = timeout(Duration::from_secs(20), backend.created.notified()).await;
    if let Some(mut stdin) = tui.stdin.take() {
        let _ = stdin.write_all(b"\x03\x03").await;
    }
    if timeout(Duration::from_secs(3), tui.wait()).await.is_err() {
        let _ = tui.kill().await;
    }
    shutdown.cancel();
    running.await??;
    boot_result.map_err(|_| "Codex TUI did not call thread/start")?;
    assert_eq!(backend.create_commands.lock().expect("test lock").len(), 1);
    Ok(())
}

/// Oracle: Codex TUI remote requirements section 2. A running Turn must emit
/// turn/started, and cancellation must settle with turn/completed interrupted.
#[tokio::test]
async fn web_socket_streams_turn_start_and_interrupt_from_hub()
-> Result<(), Box<dyn std::error::Error>> {
    use tokio::time::{Duration, timeout};

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let backend = ScriptedSessionBackend::new()?;
    let context = Arc::new(RouterSessionAppServerContext::new(
        backend.endpoint.clone(),
        ScriptedSessionBackend::actor()?,
        Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
        Arc::clone(&backend) as Arc<dyn SessionEventHub>,
    ));
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));
    let stream = UnixStream::connect(&socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;

    client
        .send(Message::Text(
            json!({"id":1,"method":"thread/start",
        "params":{"cwd":"/tmp","model":"provider-default"}})
            .to_string()
            .into(),
        ))
        .await?;
    let started: Value =
        serde_json::from_str(client.next().await.ok_or("thread reply")??.to_text()?)?;
    let thread_id = started["result"]["thread"]["id"]
        .as_str()
        .ok_or("thread id")?;
    client
        .send(Message::Text(
            json!({"id":10,"method":"thread/list",
        "params":{"sourceKinds":["cli","vscode"]}})
            .to_string()
            .into(),
        ))
        .await?;
    let listed: Value =
        serde_json::from_str(client.next().await.ok_or("thread list")??.to_text()?)?;
    assert_eq!(listed["result"]["data"][0]["id"], thread_id);
    client
        .send(Message::Text(
            json!({"id":11,"method":"thread/read",
        "params":{"threadId":thread_id,"includeTurns":false}})
            .to_string()
            .into(),
        ))
        .await?;
    let read: Value = serde_json::from_str(client.next().await.ok_or("thread read")??.to_text()?)?;
    assert_eq!(read["result"]["thread"]["id"], thread_id);
    client
        .send(Message::Text(
            json!({"id":12,"method":"thread/resume",
        "params":{"threadId":thread_id,"excludeTurns":true}})
            .to_string()
            .into(),
        ))
        .await?;
    let resumed: Value =
        serde_json::from_str(client.next().await.ok_or("thread resume")??.to_text()?)?;
    assert_eq!(resumed["result"]["thread"]["id"], thread_id);
    assert_eq!(resumed["result"]["approvalPolicy"], "on-request");
    assert_eq!(resumed["result"]["approvalsReviewer"], "user");
    client
        .send(Message::Text(
            json!({"id":2,"method":"turn/start",
        "params":{"threadId":thread_id,"input":[{"type":"text","text":"hello"}]}})
            .to_string()
            .into(),
        ))
        .await?;
    let start_reply: Value =
        serde_json::from_str(client.next().await.ok_or("turn reply")??.to_text()?)?;
    assert_eq!(start_reply["result"]["turn"]["status"], "inProgress");
    let started_notification = timeout(Duration::from_secs(1), client.next())
        .await?
        .ok_or("turn/started missing")??;
    let started_notification: Value = serde_json::from_str(started_notification.to_text()?)?;
    assert_eq!(started_notification["method"], "turn/started");
    assert_eq!(started_notification["params"]["turn"]["id"], "turn-1");

    client
        .send(Message::Text(
            json!({"id":3,"method":"turn/interrupt",
        "params":{"threadId":thread_id,"turnId":"turn-1"}})
            .to_string()
            .into(),
        ))
        .await?;
    let interrupted: Value =
        serde_json::from_str(client.next().await.ok_or("interrupt reply")??.to_text()?)?;
    assert_eq!(interrupted["result"], json!({}));
    let completed_notification = timeout(Duration::from_secs(1), client.next())
        .await?
        .ok_or("turn/completed missing")??;
    let completed_notification: Value = serde_json::from_str(completed_notification.to_text()?)?;
    assert_eq!(completed_notification["method"], "turn/completed");
    assert_eq!(
        completed_notification["params"]["turn"]["status"],
        "interrupted"
    );

    client.close(None).await?;
    shutdown.cancel();
    running.await??;
    Ok(())
}

#[tokio::test]
async fn model_list_reads_the_current_supervisor_catalog() -> Result<(), Box<dyn std::error::Error>>
{
    use tokio::sync::watch;

    let directory = tempfile::tempdir()?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let socket_path = directory.path().join("claude.sock");
    let backend = ScriptedSessionBackend::new()?;
    let (catalog_writer, catalog_reader) = watch::channel(Vec::new());
    let context = Arc::new(
        RouterSessionAppServerContext::new(
            backend.endpoint.clone(),
            ScriptedSessionBackend::actor()?,
            Arc::clone(&backend) as Arc<dyn SessionCommandPort>,
            Arc::clone(&backend) as Arc<dyn SessionEventHub>,
        )
        .with_model_catalog(catalog_reader),
    );
    let listener = RouterSessionAppServerListener::bind(&socket_path, context)?;
    let shutdown = CancellationToken::new();
    let running = tokio::spawn(listener.run(shutdown.clone()));

    let first = request_model_list(&socket_path).await?;
    assert_eq!(first["result"]["data"][0]["id"], "provider-default");
    catalog_writer.send(vec![ProviderModelEntry::try_new(
        "claude-sonnet".into(),
        "Claude Sonnet".into(),
        "Model advertised by provider".into(),
    )?])?;
    let refreshed = request_model_list(&socket_path).await?;
    assert_eq!(refreshed["result"]["data"][0]["id"], "claude-sonnet");
    assert_eq!(
        refreshed["result"]["data"][0]["displayName"],
        "Claude Sonnet"
    );
    shutdown.cancel();
    running.await??;
    Ok(())
}

async fn request_model_list(socket_path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let stream = UnixStream::connect(socket_path).await?;
    let (mut client, _) = client_async("ws://localhost/rpc", stream).await?;
    client
        .send(Message::Text(
            json!({"id":11,"method":"model/list",
        "params":{"includeHidden":true}})
            .to_string()
            .into(),
        ))
        .await?;
    let response = client.next().await.ok_or("model response missing")??;
    let value = serde_json::from_str(response.to_text()?)?;
    client.close(None).await?;
    Ok(value)
}
