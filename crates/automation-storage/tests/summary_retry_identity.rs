use agent_automation::{
    CessationEvidence, ContinuityInput, ExecutionDestination, InstructionText,
    NativeEffectEvidence, OperationId, PreparationEffect, ScheduleDefinition, SubmissionEffect,
    TimingRule,
};
use automation_storage::{AutomationStore, RunAdmission, RunDispatchIntent, ScheduleCreate};
#[tokio::test]
async fn summary_retry_keeps_same_run_and_rejects_stale_attempt()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "summary-retry-{}.sqlite",
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
    store
        .complete_run_turn::<String, String, String, String>(automation_storage::RunCompletion {
            run_id: run.clone(),
            native_turn_id: "native-run-turn".into(),
            outcome: agent_automation::WorkerOutcome::Completed { explanation: None },
            now_ms: 502000,
        })
        .await?;
    store
        .begin_required_summary::<String, String, String, String>(
            &automation_storage::SummaryAdmission {
                run_id: run.clone(),
                timeout_seconds: 900,
                now_ms: 600000,
            },
        )
        .await?;
    let current = store
        .read_run::<String, String, String, String>(&run)
        .await?;
    let previous = current.summary_attempt.ok_or("missing summary attempt")?;
    let mut stopped = previous.effects.clone();
    stopped.cessation = CessationEvidence::Confirmed;
    store
        .record_summary_progress(automation_storage::SummaryProgress {
            run_id: run.clone(),
            attempt_id: previous.attempt_id.clone(),
            phase: agent_automation::SummaryPhase::Failed,
            effects: stopped,
            target: previous.target,
            native_turn_id: previous.native_turn_id,
            explanation: Some("summary failed".into()),
        })
        .await?;
    let request = automation_storage::SummaryRecoveryRequest {
        operation_id: OperationId::generate(),
        run_id: run.clone(),
        action: automation_storage::SummaryRecoveryAction::Retry {
            timeout_seconds: 900,
        },
        now_ms: 601000,
    };
    let mut other = AutomationStore::open(&path).await?;
    let competing = automation_storage::SummaryRecoveryRequest {
        operation_id: OperationId::generate(),
        ..request.clone()
    };
    let (first, second) = tokio::join!(
        store.recover_summary::<String, String, String, String>(&request),
        other.recover_summary::<String, String, String, String>(&competing)
    );
    let (retried, winner) = match (first, second) {
        (Ok(record), Err(_)) => (record, &request),
        (Err(_), Ok(record)) => (record, &competing),
        _ => return Err("concurrent retries admitted zero or multiple summary attempts".into()),
    };
    let replay = store
        .recover_summary::<String, String, String, String>(winner)
        .await?;
    other.close().await?;
    if retried.run_id != run
        || retried.phase != agent_automation::RunPhase::SummaryRunning
        || serde_json::to_value(&retried)? != serde_json::to_value(replay)?
    {
        return Err("summary retry changed Run or replay identity".into());
    }
    let replacement = retried
        .summary_attempt
        .ok_or("missing replacement attempt")?;
    if replacement.attempt_id == previous.attempt_id {
        return Err("retry reused prior attempt identity".into());
    }
    let stale = store
        .complete_summary::<String, String>(automation_storage::SummaryCompletion {
            run_id: run.clone(),
            attempt_id: previous.attempt_id,
            native_turn_id: "summary-turn".into(),
            text: InstructionText::try_from("old summary".to_owned())?,
            now_ms: 602000,
        })
        .await?;
    if stale {
        return Err("stale attempt completed replacement".into());
    }
    let current = store
        .inspect_schedule::<String, String>(&schedule.schedule_id)
        .await?;
    if current.active_run_id != Some(run) {
        return Err("summary retry released schedule exclusion".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
