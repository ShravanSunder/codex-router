use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{AutomationStore, RunAdmission, ScheduleCreate};

#[tokio::test]
async fn missed_ticks_coalesce_and_timer_progress_prevents_duplicate_work()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a minute schedule whose caller resumes after multiple missed ticks.
    let path = std::env::temp_dir().join(format!(
        "automation-catchup-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instructions = store
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
                instruction_id: instructions.instruction_id,
                timing: TimingRule::Interval { seconds: 60 },
                enabled: true,
                destination: ExecutionDestination::FreshEachRun {
                    endpoint: "debug".into(),
                    cwd: "/work".into(),
                },
                execution_timeout_seconds: None,
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    // Act: overdue ticks create one waiting Run, not one Run per missed minute.
    let first = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 180000)
        .await?
        .ok_or("missing catch-up Run")?;
    if store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 180000)
        .await?
        .is_some()
    {
        return Err("processed ticks produced duplicate work".into());
    }
    let coalesced = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 240000)
        .await?
        .ok_or("missing coalesced Run")?;
    if first != coalesced {
        return Err("waiting backlog was not coalesced".into());
    }
    let admitted = store
        .admit_waiting_run::<String, String>(&schedule.schedule_id, 240000)
        .await?;
    if !matches!(admitted, RunAdmission::Admitted { .. }) {
        return Err("waiting Run could not be admitted".into());
    }
    let waiting = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 300000)
        .await?
        .ok_or("missing waiting successor")?;
    // Assert: active Run remains active, exactly one new waiting successor is allowed.
    if waiting == first {
        return Err("new due work reused the active Run".into());
    }
    if !matches!(store.admit_waiting_run::<String,String>(&schedule.schedule_id,300000).await?,RunAdmission::Occupied{run_id} if run_id==first)
    {
        return Err("active Run did not block successor".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
