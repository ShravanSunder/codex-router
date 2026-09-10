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

#[tokio::test]
async fn execution_mode_cannot_change_even_before_the_first_run()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "immutable-mode-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Check".to_owned())?,
            0,
        )
        .await?;
    for fresh in [false, true] {
        let owned = ExecutionDestination::OwnedThread {
            target: "B".to_owned(),
            cwd: "/isolated-test".into(),
        };
        let separate = ExecutionDestination::FreshEachRun {
            endpoint: "local".to_owned(),
            cwd: "/isolated-test".into(),
        };
        let schedule = store
            .create_schedule(&ScheduleCreate {
                operation_id: OperationId::generate(),
                definition: ScheduleDefinition {
                    instruction_id: instruction.instruction_id.clone(),
                    timing: TimingRule::Interval { seconds: 60 },
                    enabled: false,
                    destination: if fresh {
                        separate.clone()
                    } else {
                        owned.clone()
                    },
                    execution_timeout_seconds: None,
                },
                imported_continuity: ContinuityInput::None,
                now_ms: 0,
            })
            .await?;
        let mut changed = schedule.definition.clone();
        changed.destination = if fresh { owned } else { separate };
        let result = store
            .mutate_schedule(&ScheduleMutation {
                operation_id: OperationId::generate(),
                schedule_id: schedule.schedule_id.clone(),
                edit: ScheduleEdit::Replace {
                    expected_change_id: schedule.change_id.clone(),
                    definition: changed,
                },
                now_ms: 1,
            })
            .await;
        if !matches!(
            result,
            Err(automation_storage::StorageError::InvalidSchedule {
                field: "destination",
                ..
            })
        ) {
            return Err("schedule execution mode was mutable after creation".into());
        }
        let current = store
            .inspect_schedule::<String, String>(&schedule.schedule_id)
            .await?;
        if current.record.change_id != schedule.change_id
            || serde_json::to_value(&current.record.definition)?
                != serde_json::to_value(&schedule.definition)?
        {
            return Err("rejected mode edit mutated schedule".into());
        }
        let mut expected_change_id = schedule.change_id.clone();
        if fresh {
            let package = store
                .export_schedule::<String, String>(&schedule.schedule_id)
                .await?;
            let encoded = agent_automation::encode_schedule_package(&package, 1_048_576)?;
            let overwritten = store
                .import_schedule::<String, String>(&automation_storage::ScheduleImport {
                    operation_id: OperationId::generate(),
                    package_utf8: &encoded,
                    overwrite: true,
                    now_ms: 2,
                })
                .await?;
            if overwritten.record.definition.destination.execution_mode()
                != agent_automation::ExecutionMode::FreshEachRun
                || overwritten.record.definition.destination.is_prepared()
                || overwritten.record.definition.enabled
            {
                return Err("overwrite lost mode or retained local bindings".into());
            }
            expected_change_id = overwritten.record.change_id;
            let other_path = path.with_extension("destination.sqlite");
            let mut other = AutomationStore::open(&other_path).await?;
            let imported = other
                .import_schedule::<String, String>(&automation_storage::ScheduleImport {
                    operation_id: OperationId::generate(),
                    package_utf8: &encoded,
                    overwrite: false,
                    now_ms: 3,
                })
                .await?;
            if imported.record.schedule_id != schedule.schedule_id
                || imported.record.definition.destination.execution_mode()
                    != agent_automation::ExecutionMode::FreshEachRun
                || imported.record.definition.destination.is_prepared()
            {
                return Err(
                    "cross-database import did not preserve fresh mode without bindings".into(),
                );
            }
            other.close().await?;
            std::fs::remove_file(other_path)?;
            let mut wrong_mode = package;
            wrong_mode.definition.destination = ExecutionDestination::Unprepared;
            let encoded = agent_automation::encode_schedule_package(&wrong_mode, 1_048_576)?;
            if !matches!(
                store
                    .import_schedule::<String, String>(&automation_storage::ScheduleImport {
                        operation_id: OperationId::generate(),
                        package_utf8: &encoded,
                        overwrite: true,
                        now_ms: 4,
                    })
                    .await,
                Err(automation_storage::StorageError::InvalidSchedule {
                    field: "destination",
                    ..
                })
            ) {
                return Err("overwrite changed fixed mode".into());
            }
        }
        let mut allowed = schedule.definition;
        allowed.execution_timeout_seconds = Some(120);
        allowed.enabled = true;
        store
            .mutate_schedule(&ScheduleMutation {
                operation_id: OperationId::generate(),
                schedule_id: schedule.schedule_id,
                edit: ScheduleEdit::Replace {
                    expected_change_id,
                    definition: allowed,
                },
                now_ms: 2,
            })
            .await?;
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
