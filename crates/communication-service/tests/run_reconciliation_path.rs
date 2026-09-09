//! Reconciliation observes exact persisted turns, even after replacement, without native mutations.
use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, NativeEffectEvidence, OperationId,
    ScheduleDefinition, TimingRule,
};
use automation_storage::{AutomationStore, RunDispatchIntent, ScheduleCreate};
use communication_client::ControlClient;
use communication_protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
use communication_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
async fn completed_exact_turn_reconciles_after_deadline_and_generation_replacement()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise("completed", "recorded-turn", true).await
}
#[tokio::test]
async fn active_turn_stays_occupied_without_interrupting_past_deadline()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise("inProgress", "recorded-turn", false).await
}
#[tokio::test]
async fn another_completed_turn_cannot_release_execution()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    exercise("completed", "unrelated-turn", false).await
}
async fn exercise(
    status: &str,
    observed_id: &str,
    completed: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "run-reconcile-fixture-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let mut store = AutomationStore::open(&root.join("automation.sqlite")).await?;
    let service_id = "00000000-0000-4000-8000-000000000001";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":1}))?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"recorded-thread"}),
    )?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Inspect the task".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::OwnedThread {
                    target: target.clone(),
                    cwd: "/isolated-fixture".into(),
                },
                execution_timeout_seconds: Some(120),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run_id = store
        .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("run missing")?;
    store
        .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 60001)
        .await?;
    let mut effects: NativeEffectEvidence<SessionRef, CodexGeneration> = serde_json::from_value(
        json!({"target":target,"generation":generation,"clientUserMessageId":run_id,"nativeTurnId":null,"nativeSubmissionId":null,"allocation":"notRequested","resume":"notRequested","submission":"dispatching","cessation":"unconfirmed"}),
    )?;
    store
        .begin_run_dispatch::<_, EndpointRef, _, NativeSendReceipt>(RunDispatchIntent {
            run_id: run_id.clone(),
            effects: effects.clone(),
            configured_timeout_seconds: 3600,
            now_ms: 61000,
        })
        .await?;
    effects.native_turn_id = Some("recorded-turn".into());
    effects.submission = agent_automation::SubmissionEffect::Accepted;
    let receipt: NativeSendReceipt = serde_json::from_value(
        json!({"target":target,"generation":generation,"inputKind":"agent","representation":"declaredAgentText","clientUserMessageId":run_id,"resumeEffect":"notRequested","acceptance":{"kind":"nativeInputAccepted","operation":"turnStart","disposition":"startedOrSteered","turnId":"recorded-turn"}}),
    )?;
    store
        .record_run_submission::<_, EndpointRef, _, _>(automation_storage::RunSubmissionResult {
            run_id: run_id.clone(),
            effects: effects.clone(),
            outcome: automation_storage::RunSubmissionOutcome::Accepted {
                turn_id: "recorded-turn".into(),
                receipt,
            },
        })
        .await?;
    store
        .retain_run_uncertainty::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            automation_storage::RunUncertainty {
                run_id: run_id.clone(),
                effects,
            },
        )
        .await?;
    let store = Arc::new(tokio::sync::Mutex::new(store));
    let path = root.join("native.sock");
    let listener = tokio::net::UnixListener::bind(&path)?;
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadTurnsList",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".into(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    let schemas = Arc::new(codex_native_integration::NativePayloadSchemas::from_bundle(
        &bundle,
    )?);
    let gate = NativeGenerationGate::default();
    gate.activate(
        serde_json::from_value(json!({"serviceEpoch":service_id,"generation":2}))?,
        path,
        Some(schemas),
    )?;
    let identity = ServiceIdentity::new(
        service_id,
        service_id,
        &format!("sha256:{}", "a".repeat(64)),
    )?
    .with_automation_store(store.clone())
    .with_native_backend(NativeControlBackend {
        endpoint: target.endpoint,
        gate,
        codex_home: root.clone(),
    })?;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let service = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "run-reconcile-fixture", "1").await?;
    let status = status.to_owned();
    let observed_id = observed_id.to_owned();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let init: Value =
            serde_json::from_str(socket.next().await.ok_or("init missing")??.to_text()?)?;
        socket
            .send(Message::Text(
                json!({"id":init.get("id"),"result":{}}).to_string().into(),
            ))
            .await?;
        let _initialized = socket.next().await.ok_or("initialized missing")??;
        let request: Value = serde_json::from_str(
            socket
                .next()
                .await
                .ok_or("observation missing")??
                .to_text()?,
        )?;
        if request.get("method").and_then(Value::as_str) != Some("thread/turns/list")
            || request.pointer("/params/threadId").and_then(Value::as_str)
                != Some("recorded-thread")
        {
            return Err("reconcile mutated native state or selected another target".into());
        }
        socket.send(Message::Text(json!({"id":request.get("id"),"result":{"data":[{"id":observed_id,"status":status,"items":[]}],"nextCursor":null}}).to_string().into())).await?;
        if let Some(Ok(message)) =
            tokio::time::timeout(Duration::from_secs(2), socket.next()).await?
            && !message.is_close()
        {
            return Err("reconcile issued another native operation".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let result = client
        .reconcile_run(communication_protocol::RunShowRequest {
            run_id: run_id.clone(),
        })
        .await?;
    if matches!(
        result.state,
        communication_protocol::RunState::Finished { .. }
    ) != completed
    {
        return Err("reconciled Run outcome disagrees with exact terminal evidence".into());
    }
    let stored = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run_id)
        .await?;
    if !completed && stored.phase != agent_automation::RunPhase::Uncertain {
        return Err("reconciliation released or interrupted unresolved work".into());
    }
    if stored.evidence.native.generation != Some(generation) {
        return Err("observation replaced original dispatch generation".into());
    }
    client.close().await?;
    service.await??;
    tokio::time::timeout(Duration::from_secs(2), backend).await???;
    drop(store);
    for entry in std::fs::read_dir(&root)? {
        std::fs::remove_file(entry?.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
