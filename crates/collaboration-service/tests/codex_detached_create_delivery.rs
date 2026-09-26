//! A detached Codex ACP create keeps both its operation evidence and empty native binding.
use codex_acp_adapter::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use collaboration_protocol::{
    CodexGeneration, EndpointDescription, NonEmptyText, OperationId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    CodexAppServerDeliveryRoute, CodexConversationOperationRecorder, EndpointDirectory,
    NativeControlBackend, NativeGenerationGate, ProviderOperationStore, ServiceIdentity,
    SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
    UnmaterializedThreadHolder, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap, future::Future, io, os::unix::fs::DirBuilderExt, pin::Pin, sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct EmptyStoredSessions;
impl AcpStoredSessions for EmptyStoredSessions {
    fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async { Ok(json!({"sessions":[]})) })
    }
}

async fn control_call(
    reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    id: &str,
    method: &str,
    params: Value,
) -> TestResult<Value> {
    writer
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            )
            .as_bytes(),
        )
        .await?;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line)).await??;
    Ok(serde_json::from_str(&line)?)
}

#[tokio::test]
async fn detached_create_records_target_and_starts_first_message_without_resume() -> TestResult {
    let operation_id = OperationId::generate();
    let root = std::path::PathBuf::from("/tmp")
        .join(format!("codex-detached-create-{}", operation_id.as_str()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let scratch_parent = root.join("scratch");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch_parent)?;
    let scratch_scope = "session-00000000-0000-4000-8000-000000000099";
    let scratch = scratch_parent.join(scratch_scope);
    std::fs::DirBuilder::new().mode(0o700).create(&scratch)?;
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;

    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"detached-thread"
    }))?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "ThreadTurnsList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle)?);
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture Codex",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
            "schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description.clone())?;
    let gate = NativeGenerationGate::default();
    gate.activate(
        generation.clone(),
        socket_path.clone(),
        Some(Arc::clone(&schemas)),
    )?;
    let holder = Arc::new(UnmaterializedThreadHolder::new());
    let store = Arc::new(tokio::sync::Mutex::new(
        ProviderOperationStore::open(&root.join("operations.sqlite")).await?,
    ));
    let recorder = Arc::new(CodexConversationOperationRecorder::new(
        Arc::clone(&store),
        target.endpoint.clone(),
        NonEmptyText::try_from(root.join("codex-acp.sock").display().to_string())?,
    ));
    let backend_config = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: root.clone(),
    };
    let route: Arc<dyn SessionDeliveryRoute> = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        directory,
        backend_config.clone(),
        Arc::clone(&holder),
    ));
    let delivery: Arc<dyn SessionMessageDelivery> =
        Arc::new(SessionDeliveryRouter::new(vec![route]));
    let identity = ServiceIdentity::new(
        &String::from(service_id.clone()),
        &String::from(service_id),
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_endpoints(vec![description])?
    .with_native_backend(backend_config)?
    .with_provider_operation_store(Arc::clone(&store))
    .with_codex_conversation_recorder(Arc::clone(&recorder))
    .with_session_delivery(delivery);

    let (native_started_tx, native_started_rx) = tokio::sync::oneshot::channel();
    let (release_native_tx, release_native_rx) = tokio::sync::oneshot::channel();
    let backend_scratch = scratch.display().to_string();
    let native = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        let mut native_started_tx = Some(native_started_tx);
        let mut release_native_rx = Some(release_native_rx);
        for expected in ["initialize", "initialized", "thread/start", "turn/start"] {
            let frame = tokio::time::timeout(Duration::from_secs(3), wire.next())
                .await?
                .ok_or("native connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request["method"] != expected {
                return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                    format!(
                        "unexpected native method: {} (expected {expected})",
                        request["method"]
                    )
                    .into(),
                );
            }
            if expected == "initialized" {
                continue;
            }
            if expected == "thread/start" {
                native_started_tx
                    .take()
                    .ok_or("start signal missing")?
                    .send(())
                    .map_err(|_| "start observer closed")?;
                release_native_rx
                    .take()
                    .ok_or("release receiver missing")?
                    .await?;
            }
            let result = match expected {
                "initialize" => json!({}),
                "thread/start" => json!({
                    "cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":{"type":"workspaceWrite","writableRoots":[backend_scratch]},
                    "thread":{"id":"detached-thread","cwd":"/work","turns":[]}
                }),
                _ => json!({"turn":{"id":"first-turn"}}),
            };
            wire.send(Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let (acp_client, acp_server) = tokio::net::UnixStream::pair()?;
    let serving_acp = tokio::spawn(serve_acp_connection(
        acp_server,
        AcpConnectionInputs {
            backend_path: socket_path.clone(),
            generation,
            schemas,
            stored_sessions: Arc::new(EmptyStoredSessions),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder: Arc::clone(&holder) as Arc<dyn codex_acp_adapter::UnmaterializedBindingStore>,
            recorder: recorder.clone(),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (acp_read, mut acp_write) = acp_client.into_split();
    let mut acp_read = BufReader::new(acp_read);
    acp_write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(3), acp_read.read_line(&mut line)).await??;
    if serde_json::from_str::<Value>(&line)?["result"]["protocolVersion"] != 1 {
        return Err(format!("ACP initialization failed: {line}").into());
    }
    acp_write.write_all(format!("{}\n", json!({
        "jsonrpc":"2.0","id":2,"method":"session/new","params":{
            "cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{
                "operationId":operation_id,"model":"gpt-5.6-sol","effort":"medium",
                "access":"workspace-write","scratchScope":scratch_scope,"scratchPath":scratch,
                "createdBy":target,"approver":target
            }}
        }
    })).as_bytes()).await?;
    tokio::time::timeout(Duration::from_secs(3), native_started_rx).await??;

    // The native mutation has begun, but its response has not arrived when the frontend leaves.
    drop(acp_read);
    drop(acp_write);
    tokio::time::timeout(Duration::from_secs(3), serving_acp).await???;
    release_native_tx
        .send(())
        .map_err(|_| "native response waiter closed")?;

    let (control_client, control_server) = tokio::net::UnixStream::pair()?;
    let serving_control = tokio::spawn(serve_control_connection(control_server, identity));
    let (control_read, mut control_write) = control_client.into_split();
    let mut control_read = BufReader::new(control_read);
    let initialized = control_call(&mut control_read, &mut control_write, "init", "control/initialize",
        json!({"version":{"major":1,"minor":0},"client":{"name":"detached-create-test","version":"1"}})).await?;
    if initialized.get("result").is_none() {
        return Err(format!("Control initialization failed: {initialized}").into());
    }
    let waited = control_call(
        &mut control_read,
        &mut control_write,
        "wait",
        "conversation/operationWait",
        json!({"operationId":operation_id,"timeoutSeconds":3}),
    )
    .await?;
    if waited["result"]["operation"]["target"]["sessionId"] != "detached-thread"
        || waited["result"]["operation"]["reconciliation"] != "confirmed"
    {
        return Err(format!("detached create target not recorded: {waited}").into());
    }
    holder.drain_create_tasks().await;
    if !holder.contains("detached-thread") {
        return Err("detached create lost its empty native binding".into());
    }
    let sent = control_call(
        &mut control_read,
        &mut control_write,
        "message",
        "message/send",
        json!({"target":target,"message":{"kind":"humanUser","text":"hello"},
            "mode":"auto","generationGuard":null,"correlation":null}),
    )
    .await?;
    if sent["result"]["outcome"]["kind"] != "started"
        || sent["result"]["client"]["kind"] != "codexAppServer"
        || holder.contains("detached-thread")
    {
        return Err(format!("first Control message did not start native turn: {sent}").into());
    }
    drop(control_read);
    drop(control_write);
    serving_control.await??;
    native.await??;
    drop(store);
    for name in [
        "operations.sqlite",
        "operations.sqlite-wal",
        "operations.sqlite-shm",
        "native.sock",
    ] {
        let file = root.join(name);
        if file.exists() {
            std::fs::remove_file(file)?;
        }
    }
    std::fs::remove_dir(scratch)?;
    std::fs::remove_dir(scratch_parent)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
