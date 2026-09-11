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
    exercise_scheduled_run(false, PreparationOutcome::Accepted, RunScenario::Normal).await
}
#[tokio::test]
async fn busy_target_waits_without_dispatch_budget_or_steer()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(true, PreparationOutcome::Accepted, RunScenario::Normal).await
}
#[tokio::test]
async fn explicit_sdk_summary_skip_preserves_worker_result_and_releases_occupancy()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(
        false,
        PreparationOutcome::Accepted,
        RunScenario::SkipFailedSummary,
    )
    .await
}
#[tokio::test]
async fn resumed_worker_uses_frozen_inputs_after_schedule_workspace_edit()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(
        false,
        PreparationOutcome::Accepted,
        RunScenario::FrozenInputs,
    )
    .await
}
#[tokio::test]
async fn known_resume_rejection_returns_to_preparation_and_can_complete()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(
        false,
        PreparationOutcome::Accepted,
        RunScenario::ResumeRejected,
    )
    .await
}
#[derive(Clone, Copy)]
enum RunScenario {
    Normal,
    SkipFailedSummary,
    FrozenInputs,
    ResumeRejected,
}
#[derive(Clone, Copy)]
enum PreparationOutcome {
    Accepted,
    Rejected,
    ResponseLost,
}
#[tokio::test]
async fn known_native_preparation_rejection_finishes_without_execution()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(false, PreparationOutcome::Rejected, RunScenario::Normal).await
}
#[tokio::test]
async fn lost_native_preparation_response_preserves_occupancy()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise_scheduled_run(false, PreparationOutcome::ResponseLost, RunScenario::Normal).await
}
async fn exercise_scheduled_run(
    busy_first: bool,
    preparation: PreparationOutcome,
    scenario: RunScenario,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let skip_failed_summary = matches!(scenario, RunScenario::SkipFailedSummary);
    let frozen_inputs = matches!(scenario, RunScenario::FrozenInputs);
    let resume_rejected = matches!(scenario, RunScenario::ResumeRejected);
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
    let schedule_worker = identity
        .schedule_timing_worker()
        .ok_or("scheduler missing")?;
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
    let resume_uncertainty_observed = Arc::new(tokio::sync::Notify::new());
    let backend_resume_uncertainty_observed = Arc::clone(&resume_uncertainty_observed);
    let backend = tokio::spawn(async move {
        for stage in 0..if busy_first || resume_rejected { 8 } else { 7 } {
            let logical_stage = if (busy_first || resume_rejected) && stage >= 2 {
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
                _ => "thread/read",
            };
            if request.get("method").and_then(Value::as_str) != Some(expected) {
                return Err(format!("expected {expected}").into());
            }
            if matches!(logical_stage, 2 | 4 | 6)
                && request
                    .pointer("/params/includeTurns")
                    .and_then(Value::as_bool)
                    != Some(true)
            {
                return Err("history observation did not request native turn contents".into());
            }
            if logical_stage == 0 && !matches!(preparation, PreparationOutcome::Accepted) {
                if matches!(preparation, PreparationOutcome::Rejected) {
                    socket.send(Message::Text(json!({"id":request.get("id"),"error":{"code":-32602,"message":"Fixture preparation rejected"}}).to_string().into())).await?;
                }
                return Ok(());
            }
            if frozen_inputs && logical_stage == 0 {
                let encoded = request.to_string();
                if request.pointer("/params/cwd").and_then(Value::as_str) != Some("/fresh-fixture")
                    || encoded.contains("FUTURE_INSTRUCTION")
                    || encoded.contains("/future-fixture")
                {
                    return Err(
                        "allocation used edited configuration instead of captured inputs".into(),
                    );
                }
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
                6 if skip_failed_summary => {
                    json!({"thread":{"id":"summary-new-thread","turns":[{"id":"summary-turn","status":"failed","items":[]}]}})
                }
                6 => {
                    json!({"thread":{"id":"summary-new-thread","turns":[{"id":"summary-turn","status":"completed","items":[{"type":"agentMessage","id":"summary-output","text":"Build checks passed. Monitor the next scheduled run."}]}]}})
                }
                _ => {
                    json!({"thread":{"id":"scheduled-new-thread","turns":[{"id":"scheduled-turn","status":"completed","items":[{"type":"agentMessage","id":"worker-output","text":"Build checked successfully."}]}]}})
                }
            };
            let result = if resume_rejected && stage == 1 {
                json!({"thread":{"id":"scheduled-new-thread","status":{"type":"notLoaded"}}})
            } else if busy_first && stage == 1 {
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
            if resume_rejected && stage == 1 {
                let resume: Value = serde_json::from_str(
                    socket
                        .next()
                        .await
                        .ok_or("resume request missing")??
                        .to_text()?,
                )?;
                if resume.get("method").and_then(Value::as_str) != Some("thread/resume") {
                    return Err("unloaded worker did not request resume".into());
                }
                // Make the durable in-flight state observable before returning the known rejection.
                tokio::time::timeout(
                    Duration::from_secs(6),
                    backend_resume_uncertainty_observed.notified(),
                )
                .await?;
                socket.send(Message::Text(json!({"id":resume.get("id"),"error":{"code":-32602,"message":"known resume rejection"}}).to_string().into())).await?;
                continue;
            }
            if busy_first && stage == 1 {
                if let Some(Ok(message)) =
                    tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
                    && !message.is_close()
                {
                    return Err("busy target received native mutation".into());
                }
                let ids = backend_store.lock().await.observable_run_ids(None).await?;
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
                if frozen_inputs {
                    let encoded = start.to_string();
                    if !encoded.contains("Inspect the task")
                        || encoded.contains("FUTURE_INSTRUCTION")
                    {
                        return Err(
                            "execution used edited instructions instead of captured input".into(),
                        );
                    }
                }
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
    if frozen_inputs {
        // Admit with the original fresh-thread definition, then edit only future work.
        let now = chrono::Utc::now().timestamp_millis();
        store
            .lock()
            .await
            .enqueue_due_run::<SessionRef, communication_protocol::EndpointRef>(
                &schedule.schedule_id,
                now,
            )
            .await?
            .ok_or("frozen Run missing")?;
        store
            .lock()
            .await
            .admit_waiting_run::<SessionRef, communication_protocol::EndpointRef>(
                &schedule.schedule_id,
                now,
            )
            .await?;
        let future_instruction = client
            .create_instruction(InstructionCreateParams {
                operation_id: OperationId::generate(),
                text: "FUTURE_INSTRUCTION".to_owned().try_into()?,
            })
            .await?;
        let mut future = schedule.definition.clone();
        future.enabled = false;
        future.instruction_id = future_instruction.instruction_id;
        future.execution_timeout_seconds = Some(1.try_into()?);
        future.destination = communication_protocol::ExecutionDestination::FreshEachRun {
            endpoint: target.endpoint.clone(),
            cwd: "/future-fixture".into(),
        };
        client
            .update_schedule(communication_protocol::ScheduleUpdateRequest {
                operation_id: OperationId::generate(),
                schedule_id: schedule.schedule_id.clone(),
                expected_change_id: schedule.change_id.clone(),
                definition: future,
            })
            .await?;
        // The worker has never run. Replace the repository connection so its first step
        // must load persisted admission state rather than continue with a captured local value.
        let reopened = AutomationStore::open(&database).await?;
        let old = std::mem::replace(&mut *store.lock().await, reopened);
        old.close().await?;
    }
    let worker = tokio::spawn(schedule_worker.run(shutdown.clone()));
    let record=tokio::time::timeout(Duration::from_secs(6),async{
        let mut poll=tokio::time::interval(Duration::from_millis(20));
        let mut known_run=None;
        loop{
            poll.tick().await;
            let current=store.lock().await.inspect_schedule::<SessionRef,communication_protocol::EndpointRef>(&schedule.schedule_id).await?;
            if current.active_run_id.is_some(){known_run=current.active_run_id;}
            if known_run.is_none() {
                let page = client.list_runs(communication_protocol::RunListRequest { schedule_id: schedule.schedule_id.clone(), cursor: None, limit: 1.try_into().map_err(|_|automation_storage::StorageError::InvalidRecord)? }).await.map_err(|_|automation_storage::StorageError::InvalidRecord)?;
                known_run = page.records.first().map(|run|run.run_id.clone());
            }
            if let Some(run_id)=&known_run{
                let run=store.lock().await.read_run::<SessionRef,communication_protocol::EndpointRef,CodexGeneration,communication_protocol::NativeSendReceipt>(run_id).await?;
                if resume_rejected && run.phase == agent_automation::RunPhase::Uncertain {
                    resume_uncertainty_observed.notify_one();
                }
                if skip_failed_summary && run.phase == agent_automation::RunPhase::SummaryBlocked {
                    client.skip_summary(communication_protocol::RunRecoveryRequest {
                        operation_id: OperationId::generate(), run_id: run_id.clone(),
                    }).await.map_err(|_|automation_storage::StorageError::InvalidRecord)?;
                    continue;
                }
                // Resume uncertainty is in-flight until the scripted native response resolves it.
                // Only the deliberately lost-response scenario expects uncertainty as its outcome.
                if matches!(run.phase,agent_automation::RunPhase::Finished | agent_automation::RunPhase::PreparationFailed)
                    || (matches!(preparation,PreparationOutcome::ResponseLost) && run.phase == agent_automation::RunPhase::Uncertain)
                {return Ok::<_,automation_storage::StorageError>(run);}
            }
        }
    }).await??;
    if !matches!(preparation, PreparationOutcome::Accepted) {
        let expected = if matches!(preparation, PreparationOutcome::Rejected) {
            agent_automation::RunPhase::PreparationFailed
        } else {
            agent_automation::RunPhase::Uncertain
        };
        let current = store
            .lock()
            .await
            .inspect_schedule::<SessionRef, communication_protocol::EndpointRef>(
                &schedule.schedule_id,
            )
            .await?;
        if record.phase != expected
            || record.evidence.timing.is_some()
            || record.native_turn_id.is_some()
            || current.active_run_id.is_some()
                != matches!(preparation, PreparationOutcome::ResponseLost)
        {
            return Err(
                "native preparation outcome lost its no-execution/uncertainty distinction".into(),
            );
        }
        shutdown.cancel();
        worker.await?;
        client.close().await?;
        service.await??;
        backend.await??;
        drop(store);
        for entry in std::fs::read_dir(&root)? {
            std::fs::remove_file(entry?.path())?;
        }
        std::fs::remove_dir(root)?;
        return Ok(());
    }
    if record.native_turn_id.as_deref() != Some("scheduled-turn")
        || record.worker_outcome.is_none()
        || record.completed_at_ms.is_none()
        || record.summary_text.as_deref()
            != if skip_failed_summary {
                None
            } else {
                Some("Build checks passed. Monitor the next scheduled run.")
            }
    {
        return Err("worker and summary completion did not preserve result/provenance".into());
    }
    if skip_failed_summary
        && store
            .lock()
            .await
            .inspect_schedule::<SessionRef, communication_protocol::EndpointRef>(
                &schedule.schedule_id,
            )
            .await?
            .active_run_id
            .is_some()
    {
        return Err("explicit summary skip did not release confirmed stopped Run occupancy".into());
    }
    if frozen_inputs
        && record
            .evidence
            .timing
            .as_ref()
            .is_none_or(|timing| timing.effective_timeout_seconds != 120)
    {
        return Err("resumed Run used edited timeout instead of captured override".into());
    }
    let public = client
        .read_run(communication_protocol::RunShowRequest {
            run_id: record.run_id.clone(),
        })
        .await?;
    if !matches!(
        public.state,
        communication_protocol::RunState::Finished { .. }
    ) || public.summary.is_none() != skip_failed_summary
    {
        return Err("public Run snapshot lost finished worker or summary".into());
    }
    let summaries = client
        .read_run_summaries(communication_protocol::RunSummariesRequest {
            run_id: record.run_id.clone(),
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    let [summary] = summaries.records.as_slice() else {
        return Err("expected one retained summary attempt".into());
    };
    let expected_summary_state = if skip_failed_summary {
        matches!(
            summary.state,
            communication_protocol::SummaryInspectionState::Skipped
        )
    } else {
        matches!(
            summary.state,
            communication_protocol::SummaryInspectionState::Completed
        )
    };
    if summary.retry_eligible
        || !expected_summary_state
        || !summaries.coverage.latest_attempt_included
    {
        return Err("summary history lost completed attempt or advertised an unsafe retry".into());
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
