use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, EndpointDescription, MessageContent,
    MessageDelivery, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, CodexAppServerDeliveryRoute, DeliveryClientReceipt, DeliveryFuture,
    DeliveryPrecondition, DeliveryRequest, EndpointDirectory, NativeControlBackend,
    NativeGenerationGate, SessionDeliveryRoute,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::DirBuilderExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_tungstenite::tungstenite::Message;

struct AcceptingConversationRecorder;
impl codex_acp_adapter::ConversationOperationRecorder for AcceptingConversationRecorder {
    fn admit_create<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: &'a CodexGeneration,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn before_native_dispatch<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_created<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: &'a collaboration_protocol::SessionId,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn record_failure<'a>(
        &'a self,
        _: &'a collaboration_protocol::OperationId,
        _: bool,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

struct CountingEvidenceSink(AtomicUsize);

impl AttemptEvidenceSink for CountingEvidenceSink {
    fn record(
        &self,
        _: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
}

struct RecordingEvidenceSink(Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>);

impl AttemptEvidenceSink for RecordingEvidenceSink {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        let result = self
            .0
            .lock()
            .map(|mut records| records.push(evidence))
            .map_err(|_| collaboration_service::DeliveryContractError::EvidencePersistence);
        Box::pin(async move { result })
    }
}

#[tokio::test]
async fn stale_strict_generation_stops_before_evidence_or_native_io()
-> Result<(), Box<dyn std::error::Error>> {
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;
    let current: CodexGeneration = serde_json::from_value(serde_json::json!({
        "serviceEpoch":service_id,"generation":1
    }))?;
    let stale: CodexGeneration = serde_json::from_value(serde_json::json!({
        "serviceEpoch":service_id,"generation":2
    }))?;
    let gate = NativeGenerationGate::default();
    gate.activate(
        current,
        std::path::PathBuf::from("/tmp/no-native-route.sock"),
        None,
    )?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        EndpointDirectory::new(service_id),
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: std::path::PathBuf::from("/tmp"),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let sink = Arc::new(CountingEvidenceSink(AtomicUsize::new(0)));
    let request = DeliveryRequest {
        target,
        message: MessageContent::Router {
            text: "hello".to_owned().try_into()?,
        },
        mode: MessageDelivery::Auto,
        precondition: DeliveryPrecondition::EndpointGeneration { expected: stale },
        correlation: DeliveryCorrelationId::generate(),
        attempt: agent_automation::AttemptId::generate(),
    };
    let receipt = route.deliver(request, sink.as_ref()).await?;
    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: false,
            ..
        }
    ) || sink.0.load(Ordering::SeqCst) != 0
    {
        return Err("stale strict generation crossed the effect boundary".into());
    }
    Ok(())
}

#[tokio::test]
async fn native_route_records_dispatch_before_io_and_returns_caller_correlation()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "route-fixture-{}",
        agent_automation::AttemptId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadTurnsList",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description.clone())?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket_path.clone(), Some(schemas))?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id,
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.clone(),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let sink = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let observed = Arc::clone(&sink);
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialize")??
                .to_text()?,
        )?;
        socket
            .send(Message::Text(
                json!({"id":initialize["id"],"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let _: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing initialized")??
                .to_text()?,
        )?;
        let read: Value =
            serde_json::from_str(socket.next().await.ok_or("missing read")??.to_text()?)?;
        let pre_effect_valid = {
            let pre_effect = observed.0.lock().map_err(|_| "evidence lock unavailable")?;
            pre_effect.len() == 1
                && matches!(&pre_effect[0], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Dispatching && native.client_user_message_id.as_deref() == Some("caller-correlation"))
        };
        if !pre_effect_valid {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "dispatch evidence was not recorded before native I/O".into(),
            );
        }
        if read["method"] != "thread/read" {
            return Err("unexpected native inspection".into());
        }
        socket.send(Message::Text(json!({"id":read["id"],"result":{"thread":{"id":"thread-one","status":{"type":"idle"}}}}).to_string().into())).await?;
        let start: Value =
            serde_json::from_str(socket.next().await.ok_or("missing start")??.to_text()?)?;
        if start["method"] != "turn/start"
            || start["params"]["clientUserMessageId"] != "caller-correlation"
        {
            return Err("native start lost caller correlation".into());
        }
        socket
            .send(Message::Text(
                json!({"id":start["id"],"result":{"turn":{"id":"turn-one"}}})
                    .to_string()
                    .into(),
            ))
            .await?;
        Ok(())
    });
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        route.deliver(
            DeliveryRequest {
                target,
                message: MessageContent::Router {
                    text: "hello".to_owned().try_into()?,
                },
                mode: MessageDelivery::Auto,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::try_from("caller-correlation".to_owned())?,
                attempt: agent_automation::AttemptId::generate(),
            },
            sink.as_ref(),
        ),
    )
    .await??;
    server.await??;
    let records = sink.0.lock().map_err(|_| "evidence lock unavailable")?;
    if !matches!(receipt.outcome, DeliveryOutcome::StartedOrSteered)
        || !matches!(receipt.client.as_ref(), Some(DeliveryClientReceipt::CodexAppServer(native)) if matches!(&native.acceptance, collaboration_protocol::NativeSendAcceptance::NativeInputAccepted { turn_id, .. } if String::from(turn_id.clone()) == "turn-one"))
        || records.len() != 2
        || !matches!(&records[1], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Accepted && native.native_turn_id.as_deref() == Some("turn-one"))
    {
        return Err("native route did not retain acceptance evidence and receipt".into());
    }
    drop(records);
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(root)?;
    Ok(())
}

#[tokio::test]
async fn held_empty_codex_thread_rejects_steer_then_starts_first_message_on_its_connection()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use codex_acp_adapter::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
    use std::{future::Future, io, pin::Pin};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    struct EmptyStoredSessions;
    impl AcpStoredSessions for EmptyStoredSessions {
        fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
            Box::pin(async { Ok(json!({"sessions":[]})) })
        }
    }
    let root = std::path::PathBuf::from("/tmp").join(format!("held-route-{}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let scratch_scope = "session-00000000-0000-4000-8000-000000000099";
    let scratch_parent = root.join("scratch");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&scratch_parent)?;
    let scratch = scratch_parent.join(scratch_scope);
    std::fs::DirBuilder::new().mode(0o700).create(&scratch)?;
    let socket_path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"empty-thread"
    }))?;
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
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
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description.clone())?;
    let gate = NativeGenerationGate::default();
    gate.activate(
        generation.clone(),
        socket_path.clone(),
        Some(Arc::clone(&schemas)),
    )?;
    let holder = Arc::new(collaboration_service::UnmaterializedThreadHolder::new());
    let route = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.clone(),
        },
        Arc::clone(&holder),
    ));
    let evidence = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let observed = Arc::clone(&evidence);
    let backend_scratch = scratch.clone();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for method in [
            "initialize",
            "initialized",
            "thread/start",
            "thread/queue/add",
            "turn/start",
        ] {
            let frame = tokio::time::timeout(Duration::from_secs(3), wire.next())
                .await?
                .ok_or("native connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request["method"] != method {
                return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                    format!("unexpected native method: {}", request["method"]).into(),
                );
            }
            if method == "initialized" {
                continue;
            }
            if method == "turn/start" {
                let effects = observed.0.lock().map_err(|_| "evidence lock")?;
                if !effects.iter().any(|effect| {
                    matches!(effect, RouteEffectEvidence::CodexAppServer(native)
                    if native.submission == SubmissionEffect::Dispatching)
                }) {
                    return Err("held message reached native I/O before evidence".into());
                }
            }
            let result = match method {
                "initialize" => json!({}),
                "thread/start" => json!({
                    "cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":{"type":"workspaceWrite","writableRoots":[backend_scratch]},
                    "thread":{"id":"empty-thread","cwd":"/work","turns":[]}
                }),
                "thread/queue/add" => json!({"queuedSubmission":{"id":"queued-one"}}),
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
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: socket_path.clone(),
            generation,
            schemas,
            stored_sessions: Arc::new(EmptyStoredSessions),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder: holder.clone(),
            recorder: Arc::new(AcceptingConversationRecorder),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let mut response = String::new();
    read.read_line(&mut response).await?;
    write
        .write_all(
            format!(
                "{}\n",
                json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
                    "cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{
                        "operationId":collaboration_protocol::OperationId::generate(),
                        "model":"gpt-5.6-sol","effort":"medium","access":"workspace-write",
                        "scratchScope":scratch_scope,"scratchPath":scratch,
                        "createdBy":target,"approver":target
                    }}
                }})
            )
            .as_bytes(),
        )
        .await?;
    response.clear();
    tokio::time::timeout(Duration::from_secs(3), read.read_line(&mut response)).await??;
    let created: Value = serde_json::from_str(&response)?;
    if created["result"]["sessionId"] != "empty-thread" {
        return Err(format!("session/new failed: {created}").into());
    }
    write.shutdown().await?;
    serving.await??;
    if !holder.contains("empty-thread") {
        return Err("closing session/new did not hold its empty native thread".into());
    }
    let make_request = |mode| DeliveryRequest {
        target: target.clone(),
        message: MessageContent::Router {
            text: "hello".to_owned().try_into().unwrap(),
        },
        mode,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::generate(),
        attempt: agent_automation::AttemptId::generate(),
    };
    let steer = route
        .deliver(make_request(MessageDelivery::Steer), evidence.as_ref())
        .await?;
    if !matches!(steer.outcome, DeliveryOutcome::NotSubmitted { .. })
        || !holder.contains("empty-thread")
    {
        return Err("steer consumed the unmaterialized binding".into());
    }
    let queued = route
        .deliver(make_request(MessageDelivery::Queue), evidence.as_ref())
        .await?;
    if !matches!(queued.outcome, DeliveryOutcome::Queued) || !holder.contains("empty-thread") {
        return Err("queue did not use and retain the held native connection".into());
    }
    let routed: Arc<dyn SessionDeliveryRoute> = route;
    let delivery: Arc<dyn collaboration_service::SessionMessageDelivery> =
        Arc::new(collaboration_service::SessionDeliveryRouter::new(vec![
            routed,
        ]));
    let identity = collaboration_service::ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_endpoints(vec![description])
    .map_err(std::io::Error::other)?
    .with_session_delivery(delivery);
    let (control_socket, control_server) = tokio::net::UnixStream::pair()?;
    let control_task = tokio::spawn(collaboration_service::serve_control_connection(
        control_server,
        identity,
    ));
    let mut control =
        collaboration_client::ControlClient::initialize(control_socket, "held-thread-message", "1")
            .await?;
    let started = control
        .send_message(collaboration_client::MessageSendRequest {
            target: target.clone(),
            message: collaboration_client::PublicMessageContent::HumanUser {
                text: "hello".to_owned().try_into()?,
            },
            delivery: MessageDelivery::Auto,
            generation_guard: None,
            correlation: None,
        })
        .await?;
    if !matches!(started.outcome, DeliveryOutcome::Started)
        || !matches!(
            started.client,
            Some(DeliveryClientReceipt::CodexAppServer(_))
        )
        || holder.contains("empty-thread")
    {
        return Err("first message did not start through the held connection".into());
    }
    control.close().await?;
    control_task.await??;
    backend.await??;
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(scratch)?;
    std::fs::remove_dir(scratch_parent)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
