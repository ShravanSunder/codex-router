use agent_automation::{
    CessationEvidence, ContinuityInput, ExecutionDestination, InstructionText,
    NativeEffectEvidence, OperationId, PreparationEffect, ScheduleDefinition, SubmissionEffect,
    TimingRule,
};
use automation_storage::{AutomationStore, RunAdmission, RunDispatchIntent, ScheduleCreate};
#[tokio::test]
async fn exact_completed_turn_releases_owned_run_without_touching_other_work()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "run-completion-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check job".to_owned())?,
            0,
        )
        .await?;
    let schedule = store
        .create_schedule(&ScheduleCreate::<String, String> {
            operation_id: OperationId::generate(),
            definition: ScheduleDefinition {
                instruction_id: instruction.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::OwnedThread {
                    target: "B".into(),
                    cwd: "/isolated-fixture".into(),
                },
                execution_timeout_seconds: Some(120),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("missing run")?;
    if !matches!(
        store
            .admit_waiting_run::<String, String>(&schedule.schedule_id, 61000)
            .await?,
        RunAdmission::Admitted { .. }
    ) {
        return Err("missing admission".into());
    }
    let before = store
        .read_run::<String, String, String, String>(&run)
        .await?;
    if before.evidence.timing.is_some() {
        return Err("waiting consumed budget".into());
    }
    let effects = NativeEffectEvidence {
        target: Some("B".to_owned()),
        generation: Some("generation-fixture".to_owned()),
        client_user_message_id: Some(run.as_str().into()),
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::Dispatching,
        cessation: CessationEvidence::Unconfirmed,
    };
    store
        .begin_run_dispatch::<_, String, _, String>(RunDispatchIntent {
            run_id: run.clone(),
            effects: effects.clone(),
            configured_timeout_seconds: 3600,
            now_ms: 500000,
        })
        .await?;
    let mut accepted = effects;
    accepted.submission = SubmissionEffect::Accepted;
    accepted.native_turn_id = Some("native-run-turn".into());
    store
        .record_run_submission::<_, String, _, _>(automation_storage::RunSubmissionResult {
            run_id: run.clone(),
            effects: accepted,
            outcome: automation_storage::RunSubmissionOutcome::Accepted {
                turn_id: "native-run-turn".into(),
                receipt: "native-receipt".to_owned(),
            },
        })
        .await?;
    let stale = store
        .complete_run_turn::<String, String, String, String>(automation_storage::RunCompletion {
            run_id: run.clone(),
            native_turn_id: "another-turn".into(),
            outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
            now_ms: 501000,
        })
        .await?;
    if stale {
        return Err("unrelated turn completed this run".into());
    }
    if !store
        .complete_run_turn::<String, String, String, String>(automation_storage::RunCompletion {
            run_id: run.clone(),
            native_turn_id: "native-run-turn".into(),
            outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
            now_ms: 502000,
        })
        .await?
    {
        return Err("exact completed turn not recorded".into());
    }
    let read = store
        .read_run::<String, String, String, String>(&run)
        .await?;
    if read.phase != agent_automation::RunPhase::Finished
        || read.completed_at_ms != Some(502000)
        || read.worker_outcome.is_none()
    {
        return Err("owned-thread run did not finish honestly".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
