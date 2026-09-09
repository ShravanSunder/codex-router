//! Complete scheduled worker protocol path with real SQLite and scripted native sockets.
use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{
    CodexGeneration, EndpointDescription, InstructionCreateParams, InstructionText, OperationId,
    ScheduleCreateRequest, SessionRef,
};
use communication_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;
#[tokio::test]
async fn scheduled_fresh_thread_finishes_after_separate_luna_summary()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(false).await
}
#[tokio::test]
async fn busy_target_waits_without_dispatch_budget_or_steer()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(true).await
}
async fn exercise_scheduled_run(
    busy_first: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "scheduled-run-fixture-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let database = root.join("automation.sqlite");
    let store = Arc::new(tokio::sync::Mutex::new(
        AutomationStore::open(&database).await?,
    ));
    let path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&path)?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"fixture-new-target"}),
    )?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadTurnsList",
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
        json!({"endpoint":target.endpoint,"label":"Fixture","availability":{"state":"available","observedAt":"2026-09-08T00:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":schemas.schema_digest(),"generation":generation}]}),
    )?;
    let gate = NativeGenerationGate::default();
    gate.activate(generation, path.clone(), Some(schemas))?;
    let identity = ServiceIdentity::new(
        service_id,
        service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_endpoints(vec![description])?
    .with_automation_store(store.clone())
    .with_native_backend(NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: root.clone(),
    })?;
    let shutdown = tokio_util::sync::CancellationToken::new();
    let worker = tokio::spawn(
        identity
            .schedule_timing_worker()
            .ok_or("scheduler missing")?
            .run(shutdown.clone()),
    );
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "scheduled-run-fixture", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Inspect the task".to_owned())?,
        })
        .await?;
    let backend_store = Arc::clone(&store);
    let backend = tokio::spawn(async move {
        for stage in 0..if busy_first { 8 } else { 7 } {
            let logical_stage = if busy_first && stage >= 2 {
                stage - 1
            } else {
                stage
            };
            let (stream, _) = listener.accept().await?;
            let mut socket = tokio_tungstenite::accept_async(stream).await?;
            let init: Value =
                serde_json::from_str(socket.next().await.ok_or("missing init")??.to_text()?)?;
            socket
                .send(Message::Text(
                    json!({"id":init.get("id"),"result":{}}).to_string().into(),
                ))
                .await?;
            let _initialized = socket.next().await.ok_or("missing initialized")??;
            let request: Value =
                serde_json::from_str(socket.next().await.ok_or("missing request")??.to_text()?)?;
            let expected = match logical_stage {
                0 | 3 => "thread/start",
                1 => "thread/read",
                5 => "turn/start",
                _ => "thread/turns/list",
            };
            if request.get("method").and_then(Value::as_str) != Some(expected) {
                return Err(format!("expected {expected}").into());
            }
            if logical_stage == 3
                && (request.pointer("/params/model").and_then(Value::as_str)
                    != Some("gpt-5.6-luna")
                    || request.pointer("/params/sandbox").and_then(Value::as_str)
                        != Some("read-only"))
            {
                return Err("summary did not request read-only Luna".into());
            }
            if logical_stage == 5
                && request.pointer("/params/threadId").and_then(Value::as_str)
                    != Some("summary-new-thread")
            {
                return Err("summary input went to worker thread".into());
            }
            let result = match logical_stage {
                0 => {
                    json!({"thread":{"id":"scheduled-new-thread","cwd":"/fresh-fixture"},"cwd":"/fresh-fixture"})
                }
                1 => json!({"thread":{"id":"scheduled-new-thread","status":{"type":"idle"}}}),
                3 => {
                    json!({"thread":{"id":"summary-new-thread","cwd":"/fresh-fixture"},"cwd":"/fresh-fixture","model":"gpt-5.6-luna","sandbox":{"type":"readOnly"}})
                }
                5 => json!({"turn":{"id":"summary-turn"}}),
                6 => {
                    json!({"data":[{"id":"summary-turn","status":"completed","items":[{"type":"agentMessage","id":"summary-output","text":"Build checks passed. Monitor the next scheduled run."}]}],"nextCursor":null})
                }
                _ => {
                    json!({"data":[{"id":"scheduled-turn","status":"completed","items":[{"type":"agentMessage","id":"worker-output","text":"Build checked successfully."}]}],"nextCursor":null})
                }
            };
            let result = if busy_first && stage == 1 {
                json!({"thread":{"id":"scheduled-new-thread","status":{"type":"active"}}})
            } else {
                result
            };
            socket
                .send(Message::Text(
                    json!({"id":request.get("id"),"result":result})
                        .to_string()
                        .into(),
                ))
                .await?;
            if busy_first && stage == 1 {
                if let Some(Ok(message)) =
                    tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
                    && !message.is_close()
                {
                    return Err("busy target received native mutation".into());
                }
                let ids = backend_store.lock().await.observable_run_ids().await?;
                let run_id = ids.first().ok_or("busy Run missing")?;
                let run=backend_store.lock().await.read_run::<SessionRef,communication_protocol::EndpointRef,CodexGeneration,communication_protocol::NativeSendReceipt>(run_id).await?;
                if run.evidence.timing.is_some() {
                    return Err("busy wait consumed execution budget".into());
                }
                continue;
            }
            if logical_stage == 1 {
                let start: Value = serde_json::from_str(
                    socket
                        .next()
                        .await
                        .ok_or("missing turn start")??
                        .to_text()?,
                )?;
                if start.get("method").and_then(Value::as_str) != Some("turn/start") {
                    return Err("scheduled worker steered instead of starting".into());
                }
                socket
                    .send(Message::Text(
                        json!({"id":start.get("id"),"result":{"turn":{"id":"scheduled-turn"}}})
                            .to_string()
                            .into(),
                    ))
                    .await?;
            }
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let schedule=client.create_schedule(serde_json::from_value::<ScheduleCreateRequest>(json!({"operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,"timing":{"kind":"at","at":"2026-01-01T00:00:00.000Z"},"enabled":true,"destination":{"kind":"freshEachRun","endpoint":target.endpoint,"cwd":"/fresh-fixture"},"executionTimeoutSeconds":120}}))?).await?;
    let record=tokio::time::timeout(Duration::from_secs(6),async{
        let mut poll=tokio::time::interval(Duration::from_millis(20));
        let mut known_run=None;
        loop{
            poll.tick().await;
            let current=store.lock().await.inspect_schedule::<SessionRef,communication_protocol::EndpointRef>(&schedule.schedule_id).await?;
            if current.active_run_id.is_some(){known_run=current.active_run_id;}
            if let Some(run_id)=&known_run{
                let run=store.lock().await.read_run::<SessionRef,communication_protocol::EndpointRef,CodexGeneration,communication_protocol::NativeSendReceipt>(run_id).await?;
                if run.phase==agent_automation::RunPhase::Finished{return Ok::<_,automation_storage::StorageError>(run);}
            }
        }
    }).await??;
    if record.native_turn_id.as_deref() != Some("scheduled-turn")
        || record.worker_outcome.is_none()
        || record.completed_at_ms.is_none()
        || record.summary_text.as_deref()
            != Some("Build checks passed. Monitor the next scheduled run.")
    {
        return Err("worker and summary completion did not preserve result/provenance".into());
    }
    let public = client
        .read_run(communication_protocol::RunShowRequest {
            run_id: record.run_id.clone(),
        })
        .await?;
    if !matches!(
        public.state,
        communication_protocol::RunState::Finished { .. }
    ) || public.summary.is_none()
    {
        return Err("public Run snapshot lost finished worker or summary".into());
    }
    shutdown.cancel();
    worker.await?;
    client.close().await?;
    service.await??;
    backend.await??;
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
