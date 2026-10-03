use agent_automation::{
    ClaudeCodePeerEffectEvidence, PeerProcessId, PeerSessionReference, PeerWriteEffect,
    RouteEffectEvidence, SubmissionEffect,
};
#[path = "../../codex-acp-adapter/tests/support/native_permission_echo.rs"]
mod native_permission_echo;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, EndpointDescription, MessageDelivery,
    MessageInputKind, MessageRepresentation, MessageText, PushId, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, AttemptReconciliation, AttemptReconciliationContext,
    CodexAppServerDeliveryRoute, DeliveryClientReceipt, DeliveryContractError, DeliveryFuture,
    DeliveryPrecondition, EndpointDirectory, LoadPolicy, NativeControlBackend,
    NativeGenerationGate, ScheduledRunPayload, SessionDeliveryRoute, SessionDeliveryRouter,
    SessionMessageDelivery,
    layer_zero::{DeliveryRequest as PreparedDeliveryRequest, PreparedPush},
};
use futures_util::{SinkExt, StreamExt};
use native_permission_echo::applied_router_sandbox;
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

fn prepared_request(
    target: SessionRef,
    body: &str,
    mode: MessageDelivery,
) -> Result<PreparedDeliveryRequest, Box<dyn std::error::Error + Send + Sync>> {
    let push_id = PushId::try_from(agent_automation::AttemptId::generate().as_str().to_owned())?;
    let correlation = DeliveryCorrelationId::try_from(push_id.as_str().to_owned())?;
    let line = MessageText::try_from(format!(
        "✉️ sender · \"{body}\" · router://{}/push/{}",
        String::from(target.endpoint.service_id.clone()),
        push_id.as_str()
    ))?;
    Ok(PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id,
            line,
            load_policy: LoadPolicy::MayLoad,
        },
        target,
        mode,
        precondition: DeliveryPrecondition::Unpinned,
        correlation,
        attempt: agent_automation::AttemptId::generate(),
    })
}

#[tokio::test]
async fn stale_strict_generation_stops_before_evidence_or_native_io()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    let mut request = prepared_request(target, "hello", MessageDelivery::Auto)?;
    request.precondition = DeliveryPrecondition::EndpointGeneration { expected: stale };
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
async fn prepared_push_reconciles_only_on_matching_codex_route_identity_and_line()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("native.sock");
    let service_id = UuidIdentity::try_from("00000000-0000-4000-8000-000000000001".to_owned())?;
    let push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458f".to_owned())?;
    let mismatched_push_id = PushId::try_from("018f47d2-24d5-7a68-b9ec-6f759c39458e".to_owned())?;
    let line: MessageText = format!(
        "✉️ sender · \"preview\" · router://{}/push/{}",
        String::from(service_id.clone()),
        push_id.as_str()
    )
    .try_into()?;
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"thread-one"
    }))?;

    // A prepared push must reject an ID mismatch before route effects or native I/O.
    let mismatched_request_route = CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        EndpointDirectory::new(service_id.clone()),
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate: NativeGenerationGate::default(),
            codex_home: root.path().to_path_buf(),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let mismatched_sink = RecordingEvidenceSink(Mutex::new(Vec::new()));
    let mismatched_request = PreparedDeliveryRequest {
        payload: PreparedPush {
            push_id: push_id.clone(),
            line: line.clone(),
            load_policy: LoadPolicy::LoadedOnly,
        },
        target: target.clone(),
        mode: MessageDelivery::Queue,
        precondition: DeliveryPrecondition::Unpinned,
        correlation: DeliveryCorrelationId::try_from(mismatched_push_id.as_str().to_owned())?,
        attempt: agent_automation::AttemptId::generate(),
    };
    if !matches!(
        mismatched_request_route
            .deliver(mismatched_request, &mismatched_sink)
            .await,
        Err(DeliveryContractError::InvalidEvidence)
    ) || !mismatched_sink
        .0
        .lock()
        .map_err(|_| "evidence lock unavailable")?
        .is_empty()
    {
        return Err("prepared push id mismatch crossed the route boundary".into());
    }

    let listener = tokio::net::UnixListener::bind(&socket_path)?;
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
        "ThreadQueueList",
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
        service_id.clone(),
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.path().to_path_buf(),
        },
        std::sync::Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let sink = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let observed = Arc::clone(&sink);
    let native_target = target.clone();
    let native_target_session_id = String::from(target.session_id.clone());
    let native_line = line.as_str().to_owned();
    let native_push_id = push_id.as_str().to_owned();
    let mismatched_native_push_id = mismatched_push_id.as_str().to_owned();
    let (finish_sender, finish_receiver) = tokio::sync::oneshot::channel();
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
                && matches!(&pre_effect[0], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Dispatching && native.target.as_ref() == Some(&native_target) && native.client_user_message_id.as_deref() == Some(native_push_id.as_str()))
        };
        if !pre_effect_valid {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "dispatch evidence was not recorded before native I/O".into(),
            );
        }
        if read["method"] != "thread/read" {
            return Err("unexpected native inspection".into());
        }
        socket.send(Message::Text(json!({"id":read["id"],"result":{"thread":{"id":"thread-one","name":"🤖 Codex Main","status":{"type":"idle"}}}}).to_string().into())).await?;
        let queued: Value =
            serde_json::from_str(socket.next().await.ok_or("missing queue add")??.to_text()?)?;
        if queued["method"] != "thread/queue/add"
            || queued["params"]["threadId"] != native_target_session_id
            || queued["params"]["clientUserMessageId"] != native_push_id
            || queued["params"]["input"][0]["text"] != native_line
        {
            return Err("Codex route changed the prepared push id, target, or line".into());
        }
        socket
            .send(Message::Text(
                json!({"id":queued["id"],"result":{"queuedSubmission":{"id":"queued-one"}}})
                    .to_string()
                    .into(),
            ))
            .await?;

        drop(socket);
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing reconciliation initialize")??
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
                .ok_or("missing reconciliation initialized")??
                .to_text()?,
        )?;
        let listing: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing queue list")??
                .to_text()?,
        )?;
        if listing["method"] != "thread/queue/list"
            || listing["params"]["threadId"] != native_target_session_id
        {
            return Err("Codex reconciliation used a different selected target".into());
        }
        socket
            .send(Message::Text(
                json!({
                    "id":listing["id"],
                    "result":{"data":[{
                        "id":"queued-one",
                        "clientUserMessageId":native_push_id,
                        "input":[{"type":"text","text":native_line}]
                    }],"nextCursor":null}
                })
                .to_string()
                .into(),
            ))
            .await?;
        drop(socket);

        // If a recorded ID disagrees with the push link, return a queue item that matches that
        // recorded ID and line. Correct reconciliation rejects it before native I/O.
        let connection = tokio::select! {
            accepted = listener.accept() => Some(accepted?),
            _ = finish_receiver => None,
        };
        if let Some((stream, _)) = connection {
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let initialize: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing mismatched-id initialize")??
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
                    .ok_or("missing mismatched-id initialized")??
                    .to_text()?,
            )?;
            let listing: Value = serde_json::from_str(
                socket
                    .next()
                    .await
                    .ok_or("missing mismatched-id queue list")??
                    .to_text()?,
            )?;
            if listing["method"] != "thread/queue/list"
                || listing["params"]["threadId"] != native_target_session_id
            {
                return Err("mismatched-ID reconciliation used an unexpected queue query".into());
            }
            socket
                .send(Message::Text(
                    json!({
                        "id":listing["id"],
                        "result":{"data":[{
                            "id":"queued-mismatched-id",
                            "clientUserMessageId":mismatched_native_push_id,
                            "input":[{"type":"text","text":native_line}]
                        }],"nextCursor":null}
                    })
                    .to_string()
                    .into(),
                ))
                .await?;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let delivery =
        SessionDeliveryRouter::new(vec![Arc::new(route) as Arc<dyn SessionDeliveryRoute>]);
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        delivery.deliver(
            PreparedDeliveryRequest {
                payload: PreparedPush {
                    push_id: push_id.clone(),
                    line: line.clone(),
                    load_policy: LoadPolicy::LoadedOnly,
                },
                target: target.clone(),
                mode: MessageDelivery::Queue,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::try_from(push_id.as_str().to_owned())?,
                attempt: agent_automation::AttemptId::generate(),
            },
            sink.as_ref(),
        ),
    )
    .await??;
    if !matches!(receipt.outcome, DeliveryOutcome::Queued)
        || receipt.reachability != Some(collaboration_protocol::SessionReachability::CodexAppServer)
    {
        return Err("prepared push did not use the selected Codex app-server route".into());
    }
    if !matches!(receipt.client.as_ref(), Some(DeliveryClientReceipt::CodexAppServer(native)) if native.target == target && String::from(native.client_user_message_id.clone()) == push_id.as_str() && native.input_kind == MessageInputKind::Agent && native.representation == MessageRepresentation::DeclaredAgentText && matches!(&native.acceptance, collaboration_protocol::NativeSendAcceptance::QueueAccepted { submission_id } if String::from(submission_id.clone()) == "queued-one"))
    {
        return Err("prepared push did not retain its Codex route evidence and receipt".into());
    }
    let recorded = {
        let records = sink.0.lock().map_err(|_| "evidence lock unavailable")?;
        if records.len() != 2
            || !matches!(&records[1], RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Accepted && native.target.as_ref() == Some(&target) && native.client_user_message_id.as_deref() == Some(push_id.as_str()) && native.native_submission_id.as_deref() == Some("queued-one"))
        {
            return Err("prepared push did not retain its Codex route evidence and receipt".into());
        }
        records[1].clone()
    };

    let accepted = delivery
        .reconcile_attempt(AttemptReconciliationContext {
            target: target.clone(),
            prepared_push_id: push_id.clone(),
            mode: MessageDelivery::Queue,
            recorded: recorded.clone(),
        })
        .await?;
    let AttemptReconciliation::Accepted(reconciled) = accepted else {
        return Err("matching Codex push queue item was not reconciled".into());
    };
    if reconciled.outcome != DeliveryOutcome::Queued
        || !matches!(reconciled.client.as_ref(), Some(DeliveryClientReceipt::CodexAppServer(native)) if native.target == target && String::from(native.client_user_message_id.clone()) == push_id.as_str() && native.input_kind == MessageInputKind::Agent && native.representation == MessageRepresentation::DeclaredAgentText)
    {
        return Err("reconciliation changed the prepared push identity or target".into());
    }

    let mut mismatched_id_evidence = recorded.clone();
    mismatched_id_evidence
        .codex_app_server_mut()
        .ok_or("prepared push lost its Codex route evidence")?
        .client_user_message_id = Some(mismatched_push_id.as_str().to_owned());
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: target.clone(),
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: mismatched_id_evidence,
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("push id mismatch reconciled as accepted".into());
    }

    let mismatched_target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"different-thread"
    }))?;
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: mismatched_target,
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: recorded.clone(),
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("target mismatch reconciled as accepted".into());
    }

    let mismatched_route = RouteEffectEvidence::ClaudeCodePeer(ClaudeCodePeerEffectEvidence {
        session_id: PeerSessionReference::try_from("sidekick-session".to_owned())?,
        process_id: PeerProcessId::try_from(1_u32)?,
        write: PeerWriteEffect::Written,
    });
    if !matches!(
        delivery
            .reconcile_attempt(AttemptReconciliationContext {
                target: target.clone(),
                prepared_push_id: push_id.clone(),
                mode: MessageDelivery::Queue,
                recorded: mismatched_route,
            })
            .await?,
        AttemptReconciliation::StillUnknown
    ) {
        return Err("route mismatch reconciled as accepted".into());
    }

    let _ = finish_sender.send(());
    server.await??;
    Ok(())
}

#[tokio::test]
async fn retiring_during_thread_read_returns_retryable_unavailable()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = tempfile::tempdir()?;
    let socket_path = root.path().join("native.sock");
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
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?)?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, socket_path, Some(schemas))?;
    let route = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id,
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate: gate.clone(),
            codex_home: root.path().to_owned(),
        },
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    ));
    let (read_seen_tx, read_seen_rx) = tokio::sync::oneshot::channel();
    let (release_server_tx, release_server_rx) = tokio::sync::oneshot::channel();
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
        let read: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing thread/read")??
                .to_text()?,
        )?;
        if read.get("method").and_then(Value::as_str) != Some("thread/read") {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "expected thread/read before generation retirement".into(),
            );
        }
        read_seen_tx
            .send(())
            .map_err(|_| std::io::Error::other("delivery task left before thread/read"))?;
        release_server_rx
            .await
            .map_err(|_| std::io::Error::other("test did not release native fixture"))?;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let mut request = prepared_request(target, "held batch", MessageDelivery::Auto)?;
    request.payload.load_policy = LoadPolicy::LoadedOnly;
    let sink = Arc::new(CountingEvidenceSink(AtomicUsize::new(0)));
    let delivery = tokio::spawn({
        let route = Arc::clone(&route);
        let sink = Arc::clone(&sink);
        async move { route.deliver(request, sink.as_ref()).await }
    });
    tokio::time::timeout(Duration::from_secs(5), read_seen_rx).await??;
    gate.retire()?;
    let receipt = tokio::time::timeout(Duration::from_secs(5), delivery).await???;
    release_server_tx
        .send(())
        .map_err(|_| std::io::Error::other("native fixture already exited"))?;
    server.await??;

    if !matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == "unavailable"
    ) {
        return Err("retirement during thread/read was not retryable unavailable".into());
    }
    Ok(())
}

#[tokio::test]
async fn held_empty_codex_thread_rejects_steer_then_starts_first_message_on_its_connection()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(None).await
}

#[tokio::test]
async fn scheduled_run_uses_held_empty_thread_without_read_or_second_connection()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::Started)).await
}

#[tokio::test]
async fn scheduled_run_busy_binding_does_not_dispatch_and_restores_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::Busy)).await
}

#[tokio::test]
async fn scheduled_run_native_rejection_restores_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::Rejected)).await
}

#[tokio::test]
async fn scheduled_run_admission_refusal_restores_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::AdmissionRefused)).await
}

#[tokio::test]
async fn scheduled_run_stale_generation_rejects_and_finishes_binding()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::Stale)).await
}

#[derive(Clone, Copy)]
enum ScheduledScenario {
    Started,
    Busy,
    Rejected,
    AdmissionRefused,
    Stale,
}

async fn exercise_held_empty_thread(
    scenario: Option<ScheduledScenario>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use codex_acp_adapter::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
    use std::{future::Future, io, pin::Pin};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    struct EmptyStoredSessions;
    impl AcpStoredSessions for EmptyStoredSessions {
        fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
            Box::pin(async { Ok(json!({"sessions":[]})) })
        }
    }
    let run_id = agent_automation::RunId::generate();
    let run_suffix = run_id
        .as_str()
        .get(24..)
        .ok_or("run id must have an ASCII UUID suffix")?;
    let root = std::env::temp_dir().join(format!("held-{run_suffix}"));
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
    let backend_config = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: root.clone(),
    };
    let route = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id.clone(),
        directory,
        backend_config.clone(),
        Arc::clone(&holder),
    ));
    let evidence = Arc::new(RecordingEvidenceSink(Mutex::new(Vec::new())));
    let observed = Arc::clone(&evidence);
    let run_id = agent_automation::RunId::generate();
    let scheduled_push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())?;
    let expected_scheduled_push_id = scheduled_push_id.clone();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        let methods: &[&str] = match scenario {
            Some(ScheduledScenario::Started | ScheduledScenario::Rejected) => {
                &["initialize", "initialized", "thread/start", "turn/start"]
            }
            Some(
                ScheduledScenario::Busy
                | ScheduledScenario::AdmissionRefused
                | ScheduledScenario::Stale,
            ) => &["initialize", "initialized", "thread/start"],
            None => &[
                "initialize",
                "initialized",
                "thread/start",
                "thread/queue/add",
                "turn/start",
            ],
        };
        for &method in methods {
            let frame = tokio::time::timeout(Duration::from_secs(3), wire.next())
                .await?
                .ok_or("native connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request.get("method").and_then(Value::as_str) != Some(method) {
                return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                    format!("unexpected native method: {:?}", request.get("method")).into(),
                );
            }
            if method == "initialized" {
                continue;
            }
            if method == "turn/start" {
                if scenario.is_some() {
                    if request
                        .pointer("/params/clientUserMessageId")
                        .and_then(Value::as_str)
                        != Some(expected_scheduled_push_id.as_str())
                    {
                        return Err(
                            "Codex clientUserMessageId did not match the scheduled push ID".into(),
                        );
                    }
                } else {
                    let effects = observed.0.lock().map_err(|_| "evidence lock")?;
                    if !effects.iter().any(|effect| {
                        matches!(effect, RouteEffectEvidence::CodexAppServer(native)
                        if native.submission == SubmissionEffect::Dispatching)
                    }) {
                        return Err("held message reached native I/O before evidence".into());
                    }
                }
            }
            let result = match method {
                "initialize" => json!({}),
                "thread/start" => json!({
                    "cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":applied_router_sandbox(&request),
                    "thread":{"id":"empty-thread","cwd":"/work","turns":[]}
                }),
                "thread/queue/add" => json!({"queuedSubmission":{"id":"queued-one"}}),
                _ => json!({"turn":{"id":"first-turn"}}),
            };
            let response = if method == "turn/start"
                && matches!(scenario, Some(ScheduledScenario::Rejected))
            {
                json!({"id":request.get("id"),"error":{"code":-32602,"message":"fixture rejected turn"}})
            } else {
                json!({"id":request.get("id"),"result":result})
            };
            wire.send(Message::Text(response.to_string().into()))
                .await?;
        }
        if tokio::time::timeout(Duration::from_millis(25), listener.accept())
            .await
            .is_ok()
        {
            return Err("scheduled run opened a second native connection".into());
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: socket_path.clone(),
            generation,
            schemas: Arc::clone(&schemas),
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
    if created.pointer("/result/sessionId").and_then(Value::as_str) != Some("empty-thread") {
        return Err(format!("session/new failed: {created}").into());
    }
    write.shutdown().await?;
    serving.await??;
    if !holder.contains("empty-thread") {
        return Err("closing session/new did not hold its empty native thread".into());
    }
    if let Some(scenario) = scenario {
        use collaboration_service::{
            RunEvidenceDisposition, RunEvidenceSink, RunSubmission, ScheduledRunExecution,
        };
        struct ScheduledEvidenceSink {
            records: Mutex<Vec<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
            refuse_dispatch: bool,
        }
        impl RunEvidenceSink for ScheduledEvidenceSink {
            fn record(
                &self,
                evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
            ) -> DeliveryFuture<'_, RunEvidenceDisposition> {
                let dispatching = matches!(&evidence, RouteEffectEvidence::CodexAppServer(native) if native.submission == SubmissionEffect::Dispatching);
                if self
                    .records
                    .lock()
                    .map(|mut records| records.push(evidence))
                    .is_err()
                {
                    return Box::pin(async {
                        Err(collaboration_service::DeliveryContractError::EvidencePersistence)
                    });
                }
                if dispatching && self.refuse_dispatch {
                    return Box::pin(async { Ok(RunEvidenceDisposition::AdmissionRefused) });
                }
                Box::pin(async {
                    Ok(RunEvidenceDisposition::Recorded {
                        timing: agent_automation::ExecutionTiming::start(1_000, 120),
                    })
                })
            }
            fn record_stop_intent(&self) -> DeliveryFuture<'_, RunEvidenceDisposition> {
                Box::pin(async { Ok(RunEvidenceDisposition::AdmissionRefused) })
            }
        }
        let inputs = serde_json::from_value(json!({
            "scheduleChangeId":agent_automation::ChangeId::generate(),
            "instructionRevisionId":agent_automation::RevisionId::generate(),
            "instructionText":"Check work","continuity":{"kind":"none"},
            "executionConfiguration":{"destination":{"kind":"ownedThread","target":target,"cwd":"/work"},
                "executionTimeoutSeconds":120,"model":"gpt-5.6-sol","effort":"medium"}
        }))?;
        let route = collaboration_service::CodexAppServerScheduledRuns::new(
            backend_config.clone(),
            Arc::clone(&holder),
        );
        let sink = ScheduledEvidenceSink {
            records: Mutex::new(Vec::new()),
            refuse_dispatch: matches!(scenario, ScheduledScenario::AdmissionRefused),
        };
        let prepared = route
            .prepare_existing_target(&target, "/work", &sink)
            .await?;
        if matches!(scenario, ScheduledScenario::Stale) {
            let stale_generation: CodexGeneration = serde_json::from_value(json!({
                "serviceEpoch":service_id,"generation":2
            }))?;
            backend_config.gate.retire()?;
            backend_config.gate.activate(
                stale_generation,
                socket_path.clone(),
                Some(Arc::clone(&schemas)),
            )?;
        }
        let busy_binding = if matches!(scenario, ScheduledScenario::Busy) {
            use codex_acp_adapter::UnmaterializedBindingStore;
            match holder.checkout("empty-thread") {
                codex_acp_adapter::HeldBindingCheckout::Ready(binding) => Some(binding),
                _ => return Err("fixture binding was not ready".into()),
            }
        } else {
            None
        };
        let submission = route
            .submit_run(
                collaboration_service::ScheduledRunSubmission {
                    run_id,
                    target: target.clone(),
                    payload: ScheduledRunPayload::Existing {
                        prepared: PreparedPush {
                            push_id: scheduled_push_id,
                            line: MessageText::try_from("scheduled hello".to_owned())?,
                            load_policy: LoadPolicy::MayLoad,
                        },
                    },
                    precondition: DeliveryPrecondition::Unpinned,
                    inputs,
                    recorded: prepared.evidence,
                },
                &sink,
            )
            .await?;
        match scenario {
            ScheduledScenario::Started
                if matches!(submission, RunSubmission::Started(_))
                    && !holder.contains("empty-thread") => {}
            ScheduledScenario::Busy
                if matches!(submission, RunSubmission::NotStartedBusy)
                    && holder.contains("empty-thread") => {}
            ScheduledScenario::AdmissionRefused
                if matches!(submission, RunSubmission::NotStartedBusy)
                    && holder.contains("empty-thread") => {}
            ScheduledScenario::Rejected
                if matches!(submission, RunSubmission::Rejected(_))
                    && holder.contains("empty-thread") => {}
            ScheduledScenario::Stale
                if matches!(submission, RunSubmission::Rejected(_))
                    && !holder.contains("empty-thread") => {}
            _ => {
                return Err(format!(
                    "unexpected scheduled submission or holder state: {submission:?}"
                )
                .into());
            }
        }
        if matches!(scenario, ScheduledScenario::Stale) {
            let recorded = sink.records.lock().map_err(|_| "evidence lock")?;
            if !matches!(recorded.last(), Some(RouteEffectEvidence::CodexAppServer(native)) if native.submission == SubmissionEffect::NotDispatched)
            {
                return Err("stale generation omitted NotDispatched evidence".into());
            }
        }
        if matches!(
            scenario,
            ScheduledScenario::AdmissionRefused | ScheduledScenario::Rejected
        ) {
            use codex_acp_adapter::UnmaterializedBindingStore;
            let restored = match holder.checkout("empty-thread") {
                codex_acp_adapter::HeldBindingCheckout::Ready(binding) => binding,
                _ => return Err("scheduled refusal left the binding busy or missing".into()),
            };
            holder.restore(*restored);
        }
        if let Some(binding) = busy_binding {
            use codex_acp_adapter::UnmaterializedBindingStore;
            holder.restore(*binding);
        }
        backend.await??;
        std::fs::remove_file(socket_path)?;
        std::fs::remove_dir(scratch)?;
        std::fs::remove_dir(scratch_parent)?;
        std::fs::remove_dir(root)?;
        return Ok(());
    }
    let make_request = |mode| prepared_request(target.clone(), "hello", mode);
    let steer = route
        .deliver(make_request(MessageDelivery::Steer)?, evidence.as_ref())
        .await?;
    if !matches!(steer.outcome, DeliveryOutcome::NotSubmitted { .. })
        || !holder.contains("empty-thread")
    {
        return Err("steer consumed the unmaterialized binding".into());
    }
    let queued = route
        .deliver(make_request(MessageDelivery::Queue)?, evidence.as_ref())
        .await?;
    if !matches!(queued.outcome, DeliveryOutcome::Queued) || !holder.contains("empty-thread") {
        return Err("queue did not use and retain the held native connection".into());
    }
    let routed: Arc<dyn SessionDeliveryRoute> = route;
    let delivery_router = Arc::new(collaboration_service::SessionDeliveryRouter::new(vec![
        routed,
    ]));
    let delivery: Arc<dyn collaboration_service::SessionMessageDelivery> = delivery_router.clone();
    let presence: Arc<dyn collaboration_service::TargetPresenceProbe> = delivery_router;
    let automation_store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let subscription_delivery = collaboration_service::SubscriptionDeliveryService::new(
        collaboration_service::SubscriptionDeliveryServiceProps {
            board_availability: collaboration_service::BoardAvailability::Unavailable,
            push_store: Arc::clone(&automation_store),
            delivery: Arc::clone(&delivery),
            presence: Arc::clone(&presence),
            machine_identity: collaboration_service::MachineIdentity::new(
                service_id.clone(),
                Some("held-thread-route-test"),
            )?,
            clock: Arc::new(collaboration_service::SystemSubscriptionClock),
        },
    );
    subscription_delivery.start().await?;
    let identity = collaboration_service::ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&automation_store))
    .with_endpoints(vec![description])
    .map_err(std::io::Error::other)?
    .with_session_delivery(delivery)
    .with_subscription_delivery_service(subscription_delivery.clone(), presence);
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
        })
        .await?;
    if !matches!(started.receipt.outcome, DeliveryOutcome::Started)
        || !matches!(
            started.receipt.client,
            Some(DeliveryClientReceipt::CodexAppServer(_))
        )
        || holder.contains("empty-thread")
    {
        return Err("first message did not start through the held connection".into());
    }
    control.close().await?;
    control_task.await??;
    subscription_delivery.shutdown().await;
    drop(subscription_delivery);
    backend.await??;
    let automation_store = Arc::try_unwrap(automation_store)
        .map_err(|_| std::io::Error::other("service retained automation store"))?
        .into_inner();
    automation_store.close().await?;
    std::fs::remove_file(root.join("automation.sqlite"))?;
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(scratch)?;
    std::fs::remove_dir(scratch_parent)?;
    std::fs::remove_dir(root)?;
    Ok(())
}
