//! Explicit CLI summary skip crosses real Control and SQLite, preserving worker outcome.
use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, NativeEffectEvidence, OperationId,
    ScheduleDefinition, TimingRule,
};
use automation_storage::{AutomationStore, RunDispatchIntent, ScheduleCreate};
use communication_protocol::{CodexGeneration, EndpointRef, NativeSendReceipt, SessionRef};
use communication_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use serde_json::{Value, json};
use std::{os::unix::fs::DirBuilderExt, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cli_summary_skip_preserves_worker_and_releases_schedule()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "summary-skip-cli-{}",
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
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: target.endpoint.clone(),
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
        .complete_run_turn::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            automation_storage::RunCompletion {
                run_id: run_id.clone(),
                native_turn_id: "recorded-turn".into(),
                outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
                now_ms: 62000,
            },
        )
        .await?;
    store
        .begin_required_summary::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(
            &automation_storage::SummaryAdmission {
                run_id: run_id.clone(),
                timeout_seconds: 900,
                now_ms: 63000,
            },
        )
        .await?;
    let previous = store
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run_id)
        .await?
        .summary_attempt
        .ok_or("missing attempt")?;
    store
        .record_summary_progress(automation_storage::SummaryProgress {
            run_id: run_id.clone(),
            attempt_id: previous.attempt_id.clone(),
            phase: agent_automation::SummaryPhase::Failed,
            effects: previous.effects,
            target: previous.target,
            native_turn_id: previous.native_turn_id,
            explanation: Some("fixture summary failed before native submission".into()),
        })
        .await?;
    let store = Arc::new(tokio::sync::Mutex::new(store));
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(service_id, service_id, &digest)
        .map_err(std::io::Error::other)?
        .with_automation_store(Arc::clone(&store));
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)?;
    let manifest = serde_json::from_value(
        json!({"version":1,"serviceId":service_id,"serviceEpoch":service_id,
        "control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest}),
    )?;
    let publication = ManifestPublication::publish(&root, &manifest)?;
    let stop = CancellationToken::new();
    let server = tokio::spawn(listener.run(stop.clone()));
    let operation = OperationId::generate();
    let output = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
            .kill_on_drop(true)
            .args([
                "run",
                "summary-skip",
                "--run-id",
                run_id.as_str(),
                "--operation-id",
                operation.as_str(),
                "--json",
                "--service-directory",
            ])
            .arg(&root)
            .output(),
    )
    .await;
    stop.cancel();
    server.await??;
    drop(publication);
    let output = output??;
    if !output.status.success() {
        return Err(format!(
            "CLI skip failed: {}; {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let result: Value = serde_json::from_slice(&output.stdout)?;
    if result.pointer("/result/runId") != Some(&json!(run_id))
        || result.pointer("/result/state/kind") != Some(&json!("finished"))
    {
        return Err(format!("CLI lost finished Run identity: {result}").into());
    }
    let current = store
        .lock()
        .await
        .read_run::<SessionRef, EndpointRef, CodexGeneration, NativeSendReceipt>(&run_id)
        .await?;
    if !matches!(
        current.worker_outcome,
        Some(agent_automation::WorkerOutcome::Completed { .. })
    ) || current
        .summary_attempt
        .is_none_or(|attempt| attempt.phase != agent_automation::SummaryPhase::Skipped)
        || current.summary_text.is_some()
        || store
            .lock()
            .await
            .inspect_schedule::<SessionRef, EndpointRef>(&schedule.schedule_id)
            .await?
            .active_run_id
            .is_some()
    {
        return Err(
            "CLI skip lost worker outcome, fabricated summary or retained occupancy".into(),
        );
    }
    drop(store);
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
