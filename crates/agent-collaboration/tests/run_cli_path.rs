//! CLI reads real persisted Run state; native receipts here are explicit fixture data, not live proof.
use agent_automation::{
    CessationEvidence, ContinuityInput, ExecutionDestination, InstructionText,
    NativeEffectEvidence, OperationId, PreparationEffect, ScheduleDefinition, SubmissionEffect,
    TimingRule,
};
use automation_storage::{
    AutomationStore, RunCompletion, RunDispatchIntent, RunSubmissionOutcome, RunSubmissionResult,
    ScheduleCreate,
};
use codex_router_host::{CollaborationRuntime, CollaborationRuntimeInputs};
use collaboration_client::protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
use serde_json::{Value, json};
use std::os::unix::fs::DirBuilderExt;
#[tokio::test]
async fn cli_reads_finished_run_from_host_storage() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp")
        .join(format!("run-cli-{}", OperationId::generate().as_str()));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let mut store = AutomationStore::open(&root.join("automation.sqlite")).await?;
    let target: SessionRef = serde_json::from_value(
        json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture-only-thread"}),
    )?;
    let generation: CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000002","generation":1}),
    )?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check fixture".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<SessionRef, EndpointRef> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::After { seconds: 1 },
                enabled: true,
                destination: ExecutionDestination::OwnedThread {
                    target: target.clone(),
                    cwd: root.to_string_lossy().into(),
                },
                execution_timeout_seconds: None,
                model: Some("gpt-5.6-sol".into()),
                effort: Some("medium".into()),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run = store
        .enqueue_due_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 1000)
        .await?
        .ok_or("missing fixture run")?;
    store
        .admit_waiting_run::<SessionRef, EndpointRef>(&schedule.schedule_id, 1000)
        .await?;
    let mut effects = NativeEffectEvidence {
        target: Some(target.clone()),
        generation: Some(generation.clone()),
        client_user_message_id: Some(run.as_str().into()),
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::Dispatching,
        cessation: CessationEvidence::Unconfirmed,
    };
    let mut prepared = effects.clone();
    prepared.submission = SubmissionEffect::NotDispatched;
    store
        .begin_run_preparation::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            automation_storage::RunPreparationIntent {
                run_id: run.clone(),
                effects: prepared.into(),
            },
        )
        .await?;
    store
        .begin_run_dispatch::<_, EndpointRef, _, NativeSendReceipt>(RunDispatchIntent {
            run_id: run.clone(),
            effects: effects.clone().into(),
            configured_timeout_seconds: 3600,
            now_ms: 1000,
        })
        .await?;
    effects.submission = SubmissionEffect::Accepted;
    effects.native_turn_id = Some("fixture-turn".into());
    let receipt: NativeSendReceipt = serde_json::from_value(
        json!({"target":target,"generation":generation,"inputKind":"agent","representation":"declaredAgentText","clientUserMessageId":run,"resumeEffect":"notRequested","acceptance":{"kind":"nativeInputAccepted","operation":"turnStart","disposition":"startedOrSteered","turnId":"fixture-turn"}}),
    )?;
    store
        .record_run_submission::<_, EndpointRef, _, _>(RunSubmissionResult {
            run_id: run.clone(),
            effects: effects.into(),
            outcome: RunSubmissionOutcome::Accepted {
                turn_id: "fixture-turn".into(),
                receipt,
            },
        })
        .await?;
    store
        .complete_run_settlement::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            RunCompletion {
                run_id: run.clone(),
                settlement: automation_storage::RunStopIdentity::NativeTurn("fixture-turn".into()),
                outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
                now_ms: 2000,
            },
        )
        .await?;
    store.close().await?;
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("absent.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
    })
    .await?;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .args([
            "run",
            "show",
            "--run-id",
            run.as_str(),
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !output.status.success() {
        return Err(format!(
            "run CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let snapshot = value
        .pointer("/result/record")
        .ok_or("missing Run result")?;
    let _: collaboration_client::protocol::RunSnapshot = serde_json::from_value(snapshot.clone())?;
    if snapshot
        .pointer("/state/execution/kind")
        .and_then(Value::as_str)
        != Some("codexAppServer")
        || snapshot
            .pointer("/executionEvidence/route/kind")
            .and_then(Value::as_str)
            != Some("codexAppServer")
    {
        return Err("CLI lost the native run route identity".into());
    }
    for (pointer, replacement) in [
        ("/state/execution/nativeTurnId", json!("another-turn")),
        ("/state/execution/target/sessionId", json!("another-thread")),
        ("/state/execution/deadlineAt", json!("2026-09-09T23:59:59Z")),
        (
            "/executionEvidence/acceptance/client/acceptance/turnId",
            json!("another-turn"),
        ),
    ] {
        let mut contradictory = snapshot.clone();
        *contradictory
            .pointer_mut(pointer)
            .ok_or("missing evidence field")? = replacement;
        if serde_json::from_value::<collaboration_client::protocol::RunSnapshot>(contradictory)
            .is_ok()
        {
            return Err(
                format!("Run snapshot accepted contradictory evidence at {pointer}").into(),
            );
        }
    }
    if value
        .pointer("/result/record/state/kind")
        .and_then(Value::as_str)
        != Some("finished")
        || value
            .pointer("/result/record/executionEvidence/acceptance/client/acceptance/turnId")
            .and_then(Value::as_str)
            != Some("fixture-turn")
    {
        return Err("CLI lost exact completed Run evidence".into());
    }
    let summaries = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "run",
            "summaries",
            "--run-id",
            run.as_str(),
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !summaries.status.success() {
        return Err("run summaries CLI failed".into());
    }
    let summaries: Value = serde_json::from_slice(&summaries.stdout)?;
    if summaries
        .pointer("/result/page/records")
        .and_then(Value::as_array)
        .is_none_or(|records| !records.is_empty())
        || summaries
            .pointer("/result/page/coverage/earlierAttempts")
            .and_then(Value::as_str)
            != Some("mayBeUnavailable")
    {
        return Err("empty summary history omitted coverage or invented an attempt".into());
    }
    let reconciled = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "run",
            "reconcile",
            "--run-id",
            run.as_str(),
            "--json",
            "--service-directory",
        ])
        .arg(&root)
        .output()
        .await?;
    if !reconciled.status.success() {
        return Err("run reconcile CLI failed".into());
    }
    let reconciled: Value = serde_json::from_slice(&reconciled.stdout)?;
    if reconciled.pointer("/result/record") != value.pointer("/result/record") {
        return Err("reconciling finished Run changed its original outcome".into());
    }
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected fixture entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
