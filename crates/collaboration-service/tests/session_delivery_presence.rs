#![allow(clippy::expect_used)]
//! Target presence and load-policy behavior through the session delivery routes.

use agent_automation::RouteEffectEvidence;
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, EndpointDescription, MessageContent,
    MessageDelivery, SessionRef, UuidIdentity,
};
use collaboration_service::{
    AttemptEvidenceSink, CodexAppServerDeliveryRoute, DeliveryContractError, DeliveryFuture,
    DeliveryPrecondition, DeliveryRequest, EndpointDirectory, LoadPolicy, NativeControlBackend,
    NativeGenerationGate, RoutePresence, SessionDeliveryRoute, SessionDeliveryRouter,
    SessionMessageDelivery,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio_tungstenite::tungstenite::Message;

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
            .map_err(|_| DeliveryContractError::EvidencePersistence);
        Box::pin(async move { result })
    }
}

struct CodexRouteFixture {
    _root: tempfile::TempDir,
    target: SessionRef,
    listener: tokio::net::UnixListener,
    route: CodexAppServerDeliveryRoute,
}

fn codex_route_fixture(
    activate_backend: bool,
) -> Result<CodexRouteFixture, Box<dyn std::error::Error + Send + Sync>> {
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
    let description: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]
    }))?;
    let directory = EndpointDirectory::new(service_id.clone());
    directory.publish(description)?;
    let gate = NativeGenerationGate::default();
    if activate_backend {
        gate.activate(generation, socket_path, Some(schemas))?;
    }
    let route = CodexAppServerDeliveryRoute::new(
        service_id,
        directory,
        NativeControlBackend {
            endpoint: target.endpoint.clone(),
            gate,
            codex_home: root.path().to_owned(),
        },
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    Ok(CodexRouteFixture {
        _root: root,
        target,
        listener,
        route,
    })
}

async fn receive_native_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    let frame = socket.next().await.ok_or("native socket closed early")??;
    Ok(serde_json::from_str(frame.to_text()?)?)
}

async fn send_native_result(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
    id: &Value,
    result: Value,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    socket
        .send(Message::Text(
            json!({"id":id,"result":result}).to_string().into(),
        ))
        .await?;
    Ok(())
}

fn required_json_field<'a>(
    value: &'a Value,
    field: &str,
) -> Result<&'a Value, Box<dyn std::error::Error + Send + Sync>> {
    value
        .get(field)
        .ok_or_else(|| format!("native frame is missing `{field}`").into())
}

async fn expect_native_connection_closed_without_followup(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match socket.next().await {
        None | Some(Err(_)) | Some(Ok(Message::Close(_))) => Ok(()),
        Some(Ok(frame)) => Err(format!("unexpected native follow-up frame: {frame:?}").into()),
    }
}

async fn serve_thread_read_statuses(
    listener: tokio::net::UnixListener,
    statuses: Vec<Option<&'static str>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for status in statuses {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize = receive_native_frame(&mut socket).await?;
        let initialize_id = required_json_field(&initialize, "id")?;
        send_native_result(&mut socket, initialize_id, json!({})).await?;
        let _initialized = receive_native_frame(&mut socket).await?;
        let read = receive_native_frame(&mut socket).await?;
        if read.get("method").and_then(Value::as_str) != Some("thread/read") {
            return Err("presence used a non-read native operation".into());
        }
        let read_id = required_json_field(&read, "id")?;
        if let Some(status) = status {
            send_native_result(
                &mut socket,
                read_id,
                json!({"thread":{"id":"thread-one","status":{"type":status}}}),
            )
            .await?;
        } else {
            socket
                .send(Message::Text(
                    json!({"id":read_id,"error":{"code":-32600,"message":"thread not loaded: thread-one"}})
                        .to_string()
                        .into(),
                ))
                .await?;
        }
        expect_native_connection_closed_without_followup(&mut socket).await?;
    }
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn codex_presence_reads_status_and_never_mutates_the_thread()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = codex_route_fixture(true)?;
    let server = tokio::spawn(serve_thread_read_statuses(
        fixture.listener,
        vec![Some("idle"), Some("active"), Some("notLoaded"), None],
    ));

    assert_eq!(
        fixture.route.presence(&fixture.target).await?,
        RoutePresence::Running
    );
    assert_eq!(
        fixture.route.presence(&fixture.target).await?,
        RoutePresence::Running
    );
    assert_eq!(
        fixture.route.presence(&fixture.target).await?,
        RoutePresence::Wakeable
    );
    assert_eq!(
        fixture.route.presence(&fixture.target).await?,
        RoutePresence::Unreachable {
            reason: "Codex thread is missing".to_owned(),
        }
    );
    server.await??;

    let wrong_endpoint: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"cursor-local"},
        "sessionId":"thread-one"
    }))?;
    assert_eq!(
        fixture.route.presence(&wrong_endpoint).await?,
        RoutePresence::NotMine
    );
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn codex_presence_reports_unreachable_when_the_backend_gate_is_down()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = codex_route_fixture(false)?;
    assert!(matches!(
        fixture.route.presence(&fixture.target).await?,
        RoutePresence::Unreachable { .. }
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn loaded_only_refuses_not_loaded_codex_thread_without_resuming_it()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = codex_route_fixture(true)?;
    let sink = RecordingEvidenceSink(Mutex::new(Vec::new()));
    let server = tokio::spawn(async move {
        let (stream, _) = fixture.listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize = receive_native_frame(&mut socket).await?;
        let initialize_id = required_json_field(&initialize, "id")?;
        send_native_result(&mut socket, initialize_id, json!({})).await?;
        let _initialized = receive_native_frame(&mut socket).await?;
        let read = receive_native_frame(&mut socket).await?;
        if read.get("method").and_then(Value::as_str) != Some("thread/read") {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "loaded-only did not inspect thread status".into(),
            );
        }
        let read_id = required_json_field(&read, "id")?;
        send_native_result(
            &mut socket,
            read_id,
            json!({"thread":{"id":"thread-one","status":{"type":"notLoaded"}}}),
        )
        .await?;
        expect_native_connection_closed_without_followup(&mut socket).await?;
        Ok(())
    });

    let receipt = fixture
        .route
        .deliver(
            DeliveryRequest {
                target: fixture.target,
                message: MessageContent::Router {
                    text: "held notification".to_owned().try_into()?,
                },
                mode: MessageDelivery::Auto,
                load_policy: LoadPolicy::LoadedOnly,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::generate(),
                attempt: agent_automation::AttemptId::generate(),
            },
            &sink,
        )
        .await?;
    server.await??;
    assert!(matches!(
        receipt.outcome,
        DeliveryOutcome::NotSubmitted {
            retryable: true,
            ref reason,
        } if reason == "notLoaded"
    ));
    Ok(())
}

#[tokio::test]
#[allow(clippy::panic_in_result_fn)]
async fn may_load_still_resumes_not_loaded_codex_thread_for_message_send()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture = codex_route_fixture(true)?;
    let target = fixture.target.clone();
    let router = SessionDeliveryRouter::new(vec![Arc::new(fixture.route)]);
    let server = tokio::spawn(async move {
        let (stream, _) = fixture.listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let initialize = receive_native_frame(&mut socket).await?;
        let initialize_id = required_json_field(&initialize, "id")?;
        send_native_result(&mut socket, initialize_id, json!({})).await?;
        let _initialized = receive_native_frame(&mut socket).await?;
        let read = receive_native_frame(&mut socket).await?;
        if read.get("method").and_then(Value::as_str) != Some("thread/read") {
            return Err::<(), Box<dyn std::error::Error + Send + Sync>>(
                "message send did not inspect thread status".into(),
            );
        }
        let read_id = required_json_field(&read, "id")?;
        send_native_result(
            &mut socket,
            read_id,
            json!({"thread":{"id":"thread-one","status":{"type":"notLoaded"}}}),
        )
        .await?;
        let resume = receive_native_frame(&mut socket).await?;
        if resume.get("method").and_then(Value::as_str) != Some("thread/resume") {
            return Err("MayLoad did not resume the not-loaded thread".into());
        }
        let resume_id = required_json_field(&resume, "id")?;
        send_native_result(
            &mut socket,
            resume_id,
            json!({"thread":{"id":"thread-one"}}),
        )
        .await?;
        let start = receive_native_frame(&mut socket).await?;
        if start.get("method").and_then(Value::as_str) != Some("turn/start") {
            return Err("message send did not start after resume".into());
        }
        let start_id = required_json_field(&start, "id")?;
        send_native_result(&mut socket, start_id, json!({"turn":{"id":"turn-one"}})).await?;
        Ok(())
    });

    let receipt = router
        .deliver(
            DeliveryRequest {
                target,
                message: MessageContent::HumanUser {
                    text: "hello".to_owned().try_into()?,
                },
                mode: MessageDelivery::Auto,
                load_policy: LoadPolicy::MayLoad,
                precondition: DeliveryPrecondition::Unpinned,
                correlation: DeliveryCorrelationId::generate(),
                attempt: agent_automation::AttemptId::generate(),
            },
            &RecordingEvidenceSink(Mutex::new(Vec::new())),
        )
        .await?;
    server.await??;
    assert!(matches!(receipt.outcome, DeliveryOutcome::StartedOrSteered));
    Ok(())
}
