//! Real socket fixture with scripted native responses; never a live model backend.
use automation_storage::AutomationStore;
use collaboration_client::{ClientError, ControlClient};
use collaboration_protocol::{
    CodexGeneration, DeliveryReceipt, EndpointDescription, MessageContent, MessageDelivery,
    SessionMessageSendParams, SessionRef,
};
use collaboration_service::{
    CodexAppServerDeliveryRoute, NativeControlBackend, NativeGenerationGate, ServiceIdentity,
    SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery, new_service_uuid,
    serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

pub enum NativeReply {
    Result(Value),
    NotificationThenResult { notification: Value, result: Value },
    Reject,
    RejectWith { code: i64, message: &'static str },
    Disconnect,
}
pub struct NativeStep {
    pub method: &'static str,
    pub reply: NativeReply,
}
pub struct MessageScenario {
    pub delivery: MessageDelivery,
    pub steps: Vec<NativeStep>,
}
type FixtureError = Box<dyn std::error::Error + Send + Sync>;
pub async fn exercise(
    scenario: MessageScenario,
) -> Result<(Result<DeliveryReceipt, ClientError>, Vec<Value>), FixtureError> {
    let fixture_id = String::from(new_service_uuid()?);
    // Keep the Unix socket path below SUN_LEN even when the host temp prefix is long.
    let short_fixture_id = fixture_id
        .get(24..)
        .ok_or_else(|| std::io::Error::other("generated service UUID is shorter than 24 bytes"))?;
    let root = std::env::temp_dir().join(format!("mf-{short_fixture_id}"));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let socket_path = root.join("n.sock");
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"target"}),
    )?;
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
    let description: EndpointDescription = serde_json::from_value(
        json!({"endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-06T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]}),
    )?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), socket_path.clone(), Some(schemas))?;
    let native_backend = NativeControlBackend {
        codex_home: root.clone(),
        endpoint: target.endpoint.clone(),
        gate,
    };
    let automation_store = Arc::new(Mutex::new(
        AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let identity = ServiceIdentity::new(service_id, service_id)?
        .with_automation_store(Arc::clone(&automation_store))
        .with_endpoints(vec![description])?
        .with_native_backend(native_backend.clone())?;
    let route: Arc<dyn SessionDeliveryRoute> = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id.to_owned().try_into()?,
        identity.endpoint_directory(),
        native_backend,
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    ));
    let delivery: Arc<dyn SessionMessageDelivery> =
        Arc::new(SessionDeliveryRouter::new(vec![route]));
    let presence = Arc::new(RunningPresence);
    let owner = collaboration_service::SubscriptionDeliveryService::new(
        collaboration_service::SubscriptionDeliveryServiceProps {
            board_availability: collaboration_service::BoardAvailability::Unavailable,
            push_store: Arc::clone(&automation_store),
            delivery: delivery.clone(),
            presence: presence.clone(),
            machine_identity: collaboration_service::MachineIdentity::new(
                service_id.to_owned().try_into()?,
                Some("message-backend-fixture"),
            )?,
            clock: Arc::new(collaboration_service::SystemSubscriptionClock),
        },
    );
    owner.start().await?;
    let identity = identity
        .with_session_delivery(delivery)
        .with_subscription_delivery_service(owner.clone(), presence);
    let (client, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity));
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native frame")??
                .to_text()?,
        )?;
        assert_eq!(
            init.get("method").and_then(Value::as_str),
            Some("initialize")
        );
        socket
            .send(Message::Text(
                json!({"id":init.get("id").ok_or("missing initialization ID")?,"result":{}})
                    .to_string()
                    .into(),
            ))
            .await?;
        let initialized: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("missing native frame")??
                .to_text()?,
        )?;
        assert_eq!(
            initialized.get("method").and_then(Value::as_str),
            Some("initialized")
        );
        let mut requests = Vec::new();
        for step in scenario.steps {
            let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
                .await?
                .ok_or("native socket closed before expected operation")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            assert_eq!(
                request.get("method").and_then(Value::as_str),
                Some(step.method)
            );
            let response = match step.reply {
                NativeReply::Result(result) => {
                    json!({"id":request.get("id").ok_or("missing request ID")?,"result":result})
                }
                NativeReply::NotificationThenResult {
                    notification,
                    result,
                } => {
                    socket
                        .send(Message::Text(notification.to_string().into()))
                        .await?;
                    json!({"id":request.get("id").ok_or("missing request ID")?,"result":result})
                }
                NativeReply::Reject => {
                    json!({"id":request.get("id").ok_or("missing request ID")?,"error":{"code":-32602,"message":"Fixture rejection"}})
                }
                NativeReply::RejectWith { code, message } => {
                    json!({"id":request.get("id").ok_or("missing request ID")?,"error":{"code":code,"message":message}})
                }
                NativeReply::Disconnect => {
                    requests.push(request);
                    return Ok::<_, FixtureError>(requests);
                }
            };
            requests.push(request);
            socket
                .send(Message::Text(response.to_string().into()))
                .await?;
        }
        // No fallback, extra mutation or retry may follow the scripted terminal response.
        if let Some(Ok(frame)) = tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
        {
            assert!(frame.is_close(), "unexpected extra native frame: {frame}");
        }
        Ok::<_, FixtureError>(requests)
    });
    let mut client = ControlClient::initialize(client, "message-proof", "1").await?;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.send_agent_message(SessionMessageSendParams {
            target: target.clone(),
            generation_guard: Some(generation),
            message: MessageContent::Agent {
                sender: target,
                text: "A finding".to_owned().try_into()?,
            },
            mode: scenario.delivery,
        }),
    )
    .await?;
    client.close().await?;
    service.await??;
    owner.shutdown().await;
    drop(owner);
    let requests = backend.await??;
    let automation_store = Arc::try_unwrap(automation_store)
        .map_err(|_| std::io::Error::other("service retained automation store"))?
        .into_inner();
    automation_store.close().await?;
    std::fs::remove_file(root.join("automation.sqlite"))?;
    std::fs::remove_file(socket_path)?;
    std::fs::remove_dir(root)?;
    Ok((result, requests))
}

struct RunningPresence;

impl collaboration_service::TargetPresenceProbe for RunningPresence {
    fn presence(
        &self,
        _: &SessionRef,
    ) -> collaboration_service::DeliveryFuture<'_, collaboration_service::TargetPresence> {
        Box::pin(async { Ok(collaboration_service::TargetPresence::Running) })
    }
}

pub fn read(status: &str) -> NativeStep {
    NativeStep {
        method: "thread/read",
        reply: NativeReply::Result(json!({"thread":{"id":"target","status":{"type":status}}})),
    }
}
pub fn outcome_data(result: Result<DeliveryReceipt, ClientError>) -> Result<Value, &'static str> {
    match result {
        Ok(receipt) => serde_json::to_value(receipt.outcome).map_err(|_| "invalid outcome"),
        Err(_) => Err("expected a typed delivery outcome"),
    }
}
