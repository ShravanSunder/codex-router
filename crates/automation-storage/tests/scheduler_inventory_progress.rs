//! Pending early work cannot hide later schedules or Runs behind a bounded inventory page.
use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, ScheduleDefinition,
    TimingRule,
};
use automation_storage::{AutomationStore, ScheduleCreate};

#[tokio::test]
async fn bounded_scheduler_scans_reach_work_beyond_first_page()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "scheduler-progress-{}.sqlite",
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
    let mut schedules = Vec::new();
    let mut runs = Vec::new();
    for _ in 0..105 {
        let schedule = store
            .create_schedule(&ScheduleCreate::<String, String> {
                operation_id: OperationId::generate(),
                definition: ScheduleDefinition {
                    instruction_id: instruction.instruction_id.clone(),
                    timing: TimingRule::Interval { seconds: 1 },
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
        runs.push(
            store
                .enqueue_due_run::<String, String>(&schedule.schedule_id, 1000)
                .await?
                .ok_or("missing run")?,
        );
        schedules.push(schedule.schedule_id);
    }
    let first = store.waiting_schedule_ids(None).await?;
    let second = store.waiting_schedule_ids(first.last()).await?;
    if first.len() != 100 || second.len() != 5 || first.iter().any(|id| second.contains(id)) {
        return Err("waiting inventory did not advance beyond blocked first page".into());
    }
    for id in &schedules {
        store.admit_waiting_run::<String, String>(id, 1000).await?;
    }
    let first = store.observable_run_ids(None).await?;
    let second = store.observable_run_ids(first.last()).await?;
    if first.len() != 100
        || second.len() != 5
        || first.iter().any(|id| second.contains(id))
        || !runs
            .iter()
            .all(|id| first.contains(id) || second.contains(id))
    {
        return Err("observable inventory starved later Runs".into());
    }
    if !store.observable_run_ids(second.last()).await?.is_empty()
        || store.observable_run_ids(None).await?.len() != 100
    {
        return Err("inventory cannot wrap for repeated observation".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
