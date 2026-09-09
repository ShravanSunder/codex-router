use agent_automation::{
    CessationEvidence, ContinuityInput, ExecutionDestination, InstructionText,
    NativeEffectEvidence, OperationId, PreparationEffect, ScheduleDefinition, SubmissionEffect,
    TimingRule,
};
use automation_storage::{AutomationStore, RunAdmission, RunDispatchIntent, ScheduleCreate};
#[tokio::test]
async fn execution_budget_starts_at_dispatch_not_trigger_and_cannot_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "run-budget-{}.sqlite",
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
    let started = store
        .begin_run_dispatch::<_, String, _, String>(RunDispatchIntent {
            run_id: run.clone(),
            effects: effects.clone(),
            configured_timeout_seconds: 3600,
            now_ms: 500000,
        })
        .await?;
    let timing = started.timing.ok_or("dispatch budget missing")?;
    if timing.dispatch_started_at_ms != 500000
        || timing.deadline_at_ms != 620000
        || timing.effective_timeout_seconds != 120
    {
        return Err("captured override or dispatch clock incorrect".into());
    }
    if store
        .begin_run_dispatch::<_, String, _, String>(RunDispatchIntent {
            run_id: run.clone(),
            effects,
            configured_timeout_seconds: 3600,
            now_ms: 600000,
        })
        .await
        .is_ok()
    {
        return Err("second dispatch reset budget".into());
    }
    let read = store
        .read_run::<String, String, String, String>(&run)
        .await?;
    if read.evidence.timing.is_none() {
        return Err("budget and SQL projections diverged".into());
    }
    let captured_inputs = serde_json::to_value(&read.inputs)?;
    let mut rejected_effects = read.evidence.native;
    rejected_effects.submission = SubmissionEffect::Rejected;
    rejected_effects.cessation = CessationEvidence::Confirmed;
    store
        .record_run_submission::<String, String, String, String>(
            automation_storage::RunSubmissionResult {
                run_id: run.clone(),
                effects: rejected_effects,
                outcome: automation_storage::RunSubmissionOutcome::Rejected {
                    explanation: "Native turn start rejected before execution".into(),
                },
            },
        )
        .await?;
    let retained = store
        .read_run::<String, String, String, String>(&run)
        .await?;
    if retained.phase != agent_automation::RunPhase::Preparing
        || retained.evidence.timing.is_some()
        || retained.evidence.acceptance.is_some()
        || retained.worker_outcome.is_some()
        || serde_json::to_value(&retained.inputs)? != captured_inputs
    {
        return Err(
            "known rejected start must preserve preparation without an execution budget".into(),
        );
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
