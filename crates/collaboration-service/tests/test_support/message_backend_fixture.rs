//! Real API fixture with scripted native responses; never a live model backend.
use automation_storage::AutomationStore;
use codex_acp_adapter::{ApprovalBroker, ApprovalRoute};
use collaboration_client::{
    ClientError, MessageSendError, MessageSendRequest, PublicMessageContent,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryReceipt, EndpointDescription, MessageDelivery, PushMessageSendResult,
    PushRecord, SessionRef,
};
use collaboration_service::{
    CodexAppServerDeliveryRoute, NativeControlBackend, NativeGenerationGate, ServiceIdentity,
    ServiceInteractionBroker, SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
#[path = "../support/served_api.rs"]
mod served_api;

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
pub struct RecordedMessageObservation {
    pub result: Result<PushMessageSendResult, MessageSendError>,
    pub requests: Vec<Value>,
    pub stored: Option<PushRecord>,
    pub target: SessionRef,
    pub generation: CodexGeneration,
}

pub async fn exercise(
    scenario: MessageScenario,
) -> Result<(Result<DeliveryReceipt, ClientError>, Vec<Value>), FixtureError> {
    let observation = exercise_message(scenario, false).await?;
    let result = observation
        .result
        .map(|push| push.receipt)
        .map_err(|error| match error {
            MessageSendError::Preparation(source) | MessageSendError::Submission { source, .. } => {
                *source
            }
        });
    Ok((result, observation.requests))
}

pub async fn exercise_recorded_loaded(
    delivery: MessageDelivery,
) -> Result<RecordedMessageObservation, FixtureError> {
    exercise_message(
        MessageScenario {
            delivery,
            steps: Vec::new(),
        },
        true,
    )
    .await
}

async fn exercise_message(
    mut scenario: MessageScenario,
    recorded: bool,
) -> Result<RecordedMessageObservation, FixtureError> {
    // Private RAII ownership keeps the native socket short and cleans broker/store sidecars too.
    let directory = tempfile::Builder::new().prefix("mf-").tempdir()?;
    let root = directory.path().to_path_buf();
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
    let broker = if recorded {
        let scratch = root.join("scratch");
        tokio::fs::create_dir(&scratch).await?;
        let path = root.join("approval-routes.json");
        let broker = ServiceInteractionBroker::load(
            service_id.to_owned().try_into()?,
            native_backend.clone(),
            path.clone(),
        )
        .await?;
        broker
            .register_route(ApprovalRoute {
                thread_id: "target".into(),
                created_by: target.clone(),
                approver: target.clone(),
                access: collaboration_protocol::RouterAccess::WorkspaceWrite,
                scratch_path: scratch.to_string_lossy().into_owned(),
                root_message_id: None,
            })
            .await?;
        drop(broker);
        let broker = ServiceInteractionBroker::load(
            service_id.to_owned().try_into()?,
            native_backend.clone(),
            path,
        )
        .await?;
        scenario.steps = recorded_loaded_steps(scenario.delivery, &root, &scratch)?;
        Some(broker)
    } else {
        None
    };
    let identity = ServiceIdentity::new(service_id, service_id)?
        .with_automation_store(Arc::clone(&automation_store))
        .with_endpoints(vec![description])?
        .with_native_backend(native_backend.clone())?;
    let route = CodexAppServerDeliveryRoute::new(
        service_id.to_owned().try_into()?,
        identity.endpoint_directory(),
        native_backend,
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    );
    let route: Arc<dyn SessionDeliveryRoute> = Arc::new(match &broker {
        Some(broker) => route.with_approval_broker(broker),
        None => route,
    });
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
    let served = served_api::ServedApi::start(identity).await?;
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
    let client = served.client("message-proof").await?;
    let result = tokio::time::timeout(
        Duration::from_secs(3),
        client.send_message(MessageSendRequest {
            target: target.clone(),
            generation_guard: Some(generation.clone()),
            message: PublicMessageContent::Agent {
                sender: target.clone(),
                text: "A finding".to_owned().try_into()?,
            },
            delivery: scenario.delivery,
        }),
    )
    .await?;
    drop(client);
    served.stop().await?;
    owner.shutdown().await;
    drop(owner);
    let requests = backend.await?;
    let stored = if recorded {
        match requests.as_ref().ok().and_then(|requests| {
            requests.iter().find_map(|request| {
                request
                    .pointer("/params/clientUserMessageId")
                    .and_then(Value::as_str)
            })
        }) {
            Some(correlation) => {
                let push = collaboration_protocol::PushId::try_from(correlation.to_owned())?;
                automation_store.lock().await.get_push_record(&push).await
            }
            None => Ok(None),
        }
    } else {
        Ok(None)
    };
    drop(broker);
    let automation_store = Arc::try_unwrap(automation_store)
        .map_err(|_| std::io::Error::other("service retained automation store"))?
        .into_inner();
    automation_store.close().await?;
    drop(directory);
    Ok(RecordedMessageObservation {
        result,
        requests: requests?,
        stored: stored?,
        target,
        generation,
    })
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

/// Independent upstream-shaped reply: literal grants, never projected from production or RPC input.
fn recorded_loaded_steps(
    delivery: MessageDelivery,
    root: &Path,
    scratch: &Path,
) -> Result<Vec<NativeStep>, FixtureError> {
    let home = std::env::var("HOME")?;
    let mut roots = vec![
        std::fs::canonicalize(scratch)?
            .to_string_lossy()
            .into_owned(),
    ];
    for location in [
        "Library/Caches",
        ".cache",
        "Library/pnpm",
        ".local/share/pnpm",
        ".local/state/pnpm",
        ".pnpm-store",
        ".npm",
        ".yarn",
        ".bun/install/cache",
        ".cargo",
        ".rustup",
        "go/pkg",
        ".gradle",
        ".m2",
        "Library/org.swift.swiftpm",
        ".swiftpm",
        "Library/Developer",
        ".local/share/uv",
        "Library/Application Support/uv",
        ".local/share/mise",
        ".local/state/mise",
    ] {
        let symlinked = Path::new(location)
            .ancestors()
            .filter(|part| !part.as_os_str().is_empty())
            .any(|part| {
                std::fs::symlink_metadata(Path::new(&home).join(part))
                    .is_ok_and(|metadata| metadata.file_type().is_symlink())
            });
        if !symlinked {
            roots.push(format!("{home}/{location}"));
        }
    }
    let cwd = std::fs::canonicalize(root)?.to_string_lossy().into_owned();
    let status = if delivery == MessageDelivery::Steer {
        "active"
    } else {
        "idle"
    };
    let mut steps = vec![
        NativeStep {
            method: "thread/read",
            reply: NativeReply::Result(json!({"thread":{
            "id":"target","cwd":cwd,"status":{"type":status}}})),
        },
        NativeStep {
            method: "thread/resume",
            reply: NativeReply::Result(json!({
                "thread":{"id":"target","cwd":cwd},"cwd":cwd,
                "activePermissionProfile":{"id":"router-workspace-write"},
                "sandbox":{"type":"workspaceWrite","networkAccess":true,"writableRoots":roots}
            })),
        },
    ];
    match delivery {
        MessageDelivery::Queue => steps.push(NativeStep {
            method: "thread/queue/add",
            reply: NativeReply::Result(json!({"queuedSubmission":{"id":"recorded-queue-1"}})),
        }),
        MessageDelivery::Steer => steps.extend([
            NativeStep {
                method: "thread/turns/list",
                reply: NativeReply::Result(json!({
                "data":[{"id":"recorded-turn-1","status":"inProgress"}],
                "nextCursor":null,"backwardsCursor":null})),
            },
            NativeStep {
                method: "turn/steer",
                reply: NativeReply::Result(json!({"turnId":"recorded-turn-1"})),
            },
        ]),
        MessageDelivery::Auto => return Err("recorded fixture requires Queue or Steer".into()),
    }
    Ok(steps)
}
