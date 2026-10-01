use agent_automation::{RouteEffectEvidence, SubmissionEffect};
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, EndpointDescription, MessageContent,
    MessageDelivery, MessageHeaderContext, SessionRef, UuidIdentity,
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
    sync::{Arc, Mutex},
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

#[tokio::test]
async fn codex_presence_reports_held_unmaterialized_thread_as_running()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::Presence)).await
}

#[tokio::test]
async fn codex_presence_reports_busy_held_thread_as_running()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_held_empty_thread(Some(ScheduledScenario::PresenceBusy)).await
}

#[derive(Clone, Copy)]
enum ScheduledScenario {
    Started,
    Busy,
    Rejected,
    AdmissionRefused,
    Stale,
    Presence,
    PresenceBusy,
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
    let short_run_id = run_id
        .as_str()
        .get(24..)
        .ok_or_else(|| std::io::Error::other("generated RunId is shorter than 24 bytes"))?;
    let root = std::env::temp_dir().join(format!("held-{short_run_id}"));
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
    let backend_scratch = scratch.clone();
    let run_id = agent_automation::RunId::generate();
    let expected_run_id = run_id.clone();
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
                | ScheduledScenario::Stale
                | ScheduledScenario::Presence
                | ScheduledScenario::PresenceBusy,
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
                        != Some(expected_run_id.as_str())
                    {
                        return Err("scheduled run omitted run ID correlation".into());
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
                    "sandbox":{"type":"workspaceWrite","writableRoots":[backend_scratch]},
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
    if matches!(
        scenario,
        Some(ScheduledScenario::Presence | ScheduledScenario::PresenceBusy)
    ) {
        let busy_binding = if matches!(scenario, Some(ScheduledScenario::PresenceBusy)) {
            use codex_acp_adapter::UnmaterializedBindingStore;
            match holder.checkout("empty-thread") {
                codex_acp_adapter::HeldBindingCheckout::Ready(binding) => Some(binding),
                _ => return Err("fixture held binding was not ready".into()),
            }
        } else {
            None
        };
        let presence = route.presence(&target).await?;
        if presence != collaboration_service::RoutePresence::Running
            || !holder.contains("empty-thread")
        {
            return Err("held unmaterialized Codex thread was not reported as running".into());
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
                    message: "scheduled hello".to_owned().try_into()?,
                    header_context: collaboration_protocol::MessageHeaderContext::default(),
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
    let message_text: collaboration_protocol::MessageText = "hello".to_owned().try_into()?;
    let make_request = |mode| DeliveryRequest {
        target: target.clone(),
        message: MessageContent::Router {
            text: message_text.clone(),
        },
        header_context: MessageHeaderContext::default(),
        mode,
        load_policy: collaboration_service::LoadPolicy::MayLoad,
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
    let automation_store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&root.join("automation.sqlite")).await?,
    ));
    let identity = collaboration_service::ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&automation_store))
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
