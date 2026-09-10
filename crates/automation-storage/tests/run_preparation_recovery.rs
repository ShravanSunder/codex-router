//! Preparation failures release occupancy only with positive evidence that native work never started.
use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, NativeEffectEvidence, OperationId,
    ScheduleDefinition, TimingRule,
};
use automation_storage::{AutomationStore, RunPreparationIntent, ScheduleCreate};
use serde_json::json;

#[tokio::test]
async fn known_preparation_rejection_finishes_but_unknown_effects_do_not()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "preparation-recovery-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check task".to_owned())?,
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
                    endpoint: "fixture-endpoint".into(),
                    cwd: "/private-fixture".into(),
                },
                execution_timeout_seconds: None,
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run_id = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("missing run")?;
    store
        .admit_waiting_run::<String, String>(&schedule.schedule_id, 60000)
        .await?;
    let mut effects: NativeEffectEvidence<String, String> = serde_json::from_value(
        json!({"target":null,"generation":"generation-1","clientUserMessageId":null,"nativeTurnId":null,"nativeSubmissionId":null,"allocation":"unknown","resume":"notRequested","submission":"notDispatched","cessation":"notApplicable"}),
    )?;
    store
        .begin_run_preparation::<_, String, _, String>(RunPreparationIntent {
            run_id: run_id.clone(),
            effects: effects.clone(),
        })
        .await?;
    let attempt = automation_storage::RunPreparationFailure {
        run_id: run_id.clone(),
        effects: effects.clone(),
        explanation: "connection lost".into(),
        now_ms: 61000,
    };
    if store
        .fail_run_preparation::<_, String, _, String>(attempt)
        .await
        .is_ok()
    {
        return Err("unknown allocation released execution".into());
    }
    effects.allocation = agent_automation::PreparationEffect::Rejected;
    store
        .fail_run_preparation::<_, String, _, String>(automation_storage::RunPreparationFailure {
            run_id: run_id.clone(),
            effects,
            explanation: "native allocation rejected".into(),
            now_ms: 62000,
        })
        .await?;
    let record = store
        .read_run::<String, String, String, String>(&run_id)
        .await?;
    if record.phase != agent_automation::RunPhase::PreparationFailed
        || record.completed_at_ms != Some(62000)
        || record.evidence.timing.is_some()
    {
        return Err("known no-execution outcome did not finish without consuming budget".into());
    }
    if store
        .inspect_schedule::<String, String>(&schedule.schedule_id)
        .await?
        .active_run_id
        .is_some()
    {
        return Err("known preparation rejection retained execution occupancy".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
