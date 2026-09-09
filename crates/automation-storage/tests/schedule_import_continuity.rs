use agent_automation::{
    ContinuityInput, ExecutionDestination, InstructionText, OperationId, PortableContinuity,
    RunPhase, ScheduleDefinition, TimingRule, encode_schedule_package,
};
use automation_storage::{AutomationStore, RunAdmission, ScheduleCreate, ScheduleImport};

#[tokio::test]
async fn overwrite_preserves_admitted_inputs_and_waiting_identity_after_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "import-active-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Inspect original workspace".to_owned())?,
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
                    endpoint: "original-endpoint".into(),
                    cwd: "/original-workspace".into(),
                },
                execution_timeout_seconds: Some(120),
            },
            imported_continuity: ContinuityInput::None,
            now_ms: 0,
        })
        .await?;
    let active = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 60_000)
        .await?
        .ok_or("missing first run")?;
    if !matches!(
        store
            .admit_waiting_run::<String, String>(&schedule.schedule_id, 60_000)
            .await?,
        RunAdmission::Admitted { .. }
    ) {
        return Err("first run was not admitted".into());
    }
    let before = serde_json::to_value(
        store
            .read_run::<String, String, String, String>(&active)
            .await?,
    )?;
    let waiting = store
        .enqueue_due_run::<String, String>(&schedule.schedule_id, 120_000)
        .await?
        .ok_or("missing waiting run")?;
    let mut package = store
        .export_schedule::<String, String>(&schedule.schedule_id)
        .await?;
    package.definition.execution_timeout_seconds = Some(300);
    package.continuity = Some(PortableContinuity {
        text: InstructionText::try_from("Imported context for future work".to_owned())?,
        source_run_id: "foreign-run".into(),
        source_target: "foreign-thread".into(),
    });
    let encoded = encode_schedule_package(&package, 1_048_576)?;
    let imported = store
        .import_schedule::<String, String>(&ScheduleImport {
            operation_id: OperationId::generate(),
            package_utf8: &encoded,
            overwrite: true,
            now_ms: 130_000,
        })
        .await?;
    if imported.active_run_id.as_ref() != Some(&active)
        || imported.waiting_run_id.as_ref() != Some(&waiting)
        || imported.record.definition.enabled
    {
        return Err("overwrite changed existing run identities or enabled future triggers".into());
    }
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    if serde_json::to_value(
        store
            .read_run::<String, String, String, String>(&active)
            .await?,
    )? != before
    {
        return Err("import/restart changed frozen active run inputs".into());
    }
    let waiting_record = store
        .read_run::<String, String, String, String>(&waiting)
        .await?;
    if waiting_record.phase != RunPhase::Waiting || waiting_record.inputs.is_some() {
        return Err("import admitted or rewrote waiting work".into());
    }
    if !matches!(store.admit_waiting_run::<String,String>(&schedule.schedule_id, 140_000).await?, RunAdmission::Occupied { run_id } if run_id == active)
    {
        return Err("import bypassed active run exclusion".into());
    }
    let exported = store
        .export_schedule::<String, String>(&schedule.schedule_id)
        .await?;
    if exported
        .continuity
        .as_ref()
        .map(|summary| summary.text.as_str())
        != Some("Imported context for future work")
    {
        return Err("imported continuity did not survive restart/export".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
