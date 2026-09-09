use agent_automation::{
    CessationEvidence, ContinuityInput, ExecutionDestination, InstructionText,
    NativeEffectEvidence, OperationId, PreparationEffect, ScheduleDefinition, SubmissionEffect,
    TimingRule,
};
use automation_storage::{AutomationStore, RunAdmission, RunDispatchIntent, ScheduleCreate};
#[tokio::test]
async fn fresh_run_keeps_occupancy_while_separate_summary_budget_begins()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "run-summary-{}.sqlite",
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
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: "fixture".into(),
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
    if read.phase != agent_automation::RunPhase::SummaryRequired || read.completed_at_ms.is_some() {
        return Err("fresh execution released slot before summary".into());
    }
    let summary = store
        .begin_required_summary::<String, String, String, String>(
            &automation_storage::SummaryAdmission {
                run_id: run.clone(),
                timeout_seconds: 900,
                now_ms: 600000,
            },
        )
        .await?;
    if summary.started_at_ms != 600000 || summary.deadline_at_ms != 1500000 {
        return Err("summary did not receive independent 15-minute budget".into());
    }
    let current = store
        .inspect_schedule::<String, String>(&schedule.schedule_id)
        .await?;
    if current.active_run_id != Some(run) {
        return Err("summary admission lost occupied Run".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
