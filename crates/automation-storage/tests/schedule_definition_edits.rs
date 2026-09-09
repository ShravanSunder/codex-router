use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{AutomationStore, ScheduleCreate, ScheduleEdit, ScheduleMutation};
#[tokio::test]
async fn disable_preserves_waiting_run_and_reenable_keeps_anchor()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-edits-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check status".to_owned())?,
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
                    cwd: "/isolated-test".into(),
                },
                execution_timeout_seconds: None,
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let run = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60000)
        .await?
        .ok_or("run not queued")?;
    let disabled = store
        .mutate_schedule(&ScheduleMutation::<String, String> {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id.clone(),
            edit: ScheduleEdit::SetEnabled { enabled: false },
            now_ms: 61000,
        })
        .await?;
    let current = store
        .inspect_schedule::<String, String>(&schedule.schedule_id)
        .await?;
    if disabled.record.definition.enabled
        || current.waiting_run_id != Some(run)
        || current.record.next_due_at_ms.is_some()
    {
        return Err("disable mixed trigger and run ownership".into());
    }
    let request = ScheduleMutation::<String, String> {
        operation_id: OperationId::generate(),
        schedule_id: schedule.schedule_id.clone(),
        edit: ScheduleEdit::SetEnabled { enabled: true },
        now_ms: 121000,
    };
    let enabled = store.mutate_schedule(&request).await?;
    let replay = store.mutate_schedule(&request).await?;
    if serde_json::to_value(&enabled)? != serde_json::to_value(replay)?
        || enabled.record.anchor_at_ms != 0
        || enabled.record.next_due_at_ms != Some(180000)
    {
        return Err("reenable replay changed original anchor".into());
    }
    let stale = store
        .mutate_schedule(&ScheduleMutation {
            operation_id: OperationId::generate(),
            schedule_id: schedule.schedule_id,
            edit: ScheduleEdit::Replace {
                expected_change_id: schedule.change_id,
                definition: enabled.record.definition,
            },
            now_ms: 122000,
        })
        .await;
    if stale.is_ok() {
        return Err("stale schedule edit was accepted".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
