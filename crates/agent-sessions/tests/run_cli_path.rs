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
use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
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
    store
        .begin_run_dispatch::<_, EndpointRef, _, NativeSendReceipt>(RunDispatchIntent {
            run_id: run.clone(),
            effects: effects.clone(),
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
            effects,
            outcome: RunSubmissionOutcome::Accepted {
                turn_id: "fixture-turn".into(),
                receipt,
            },
        })
        .await?;
    store
        .complete_run_turn::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            RunCompletion {
                run_id: run.clone(),
                native_turn_id: "fixture-turn".into(),
                outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
                now_ms: 2000,
            },
        )
        .await?;
    store.close().await?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("absent.sock"),
        native_schema: None,
    })
    .await?;
    let output = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
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
    if value.pointer("/result/state/kind").and_then(Value::as_str) != Some("finished")
        || value
            .pointer("/result/executionEvidence/acceptance/acceptance/turnId")
            .and_then(Value::as_str)
            != Some("fixture-turn")
    {
        return Err("CLI lost exact completed Run evidence".into());
    }
    let summaries = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
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
        .pointer("/result/records")
        .and_then(Value::as_array)
        .is_none_or(|records| !records.is_empty())
        || summaries
            .pointer("/result/coverage/earlierAttempts")
            .and_then(Value::as_str)
            != Some("mayBeUnavailable")
    {
        return Err("empty summary history omitted coverage or invented an attempt".into());
    }
    let reconciled = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
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
    if reconciled.get("result") != value.get("result") {
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
