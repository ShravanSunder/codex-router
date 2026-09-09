use agent_automation::{
    ExecutionDestination, InstructionId, InstructionText, OperationId, PortableInstruction,
    PortableSchedulePackage, ScheduleDefinition, ScheduleId, TimingRule, encode_schedule_package,
};
use automation_storage::{AutomationStore, ScheduleImport, StorageError};
#[tokio::test]
async fn import_preserves_uuid_requires_overwrite_and_rejects_instruction_collisions()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-import-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction_id = InstructionId::generate();
    let mut package = PortableSchedulePackage::<String, String> {
        schedule_id: ScheduleId::generate(),
        source_change_id: "source-change".into(),
        instruction: PortableInstruction {
            instruction_id: instruction_id.clone(),
            text: InstructionText::try_from("Check original job".to_owned())?,
            source_revision_id: "source-revision".into(),
        },
        definition: ScheduleDefinition {
            instruction_id,
            timing: TimingRule::Interval { seconds: 600 },
            enabled: false,
            destination: ExecutionDestination::Unprepared,
            execution_timeout_seconds: None,
        },
        continuity: None,
    };
    let text = encode_schedule_package(&package, 1_048_576)?;
    let request = ScheduleImport {
        operation_id: OperationId::generate(),
        package_utf8: &text,
        overwrite: false,
        now_ms: 1000,
    };
    let first = store.import_schedule::<String, String>(&request).await?;
    let replay = store.import_schedule::<String, String>(&request).await?;
    if first.record.schedule_id != package.schedule_id
        || first.record.definition.enabled
        || serde_json::to_value(&first)? != serde_json::to_value(replay)?
    {
        return Err("import identity/disabled/replay boundary failed".into());
    }
    if !matches!(
        store
            .import_schedule::<String, String>(&ScheduleImport {
                operation_id: OperationId::generate(),
                ..request
            })
            .await,
        Err(StorageError::ScheduleImportExists)
    ) {
        return Err("existing UUID imported without overwrite".into());
    }
    let overwrite = store
        .import_schedule::<String, String>(&ScheduleImport {
            operation_id: OperationId::generate(),
            package_utf8: &text,
            overwrite: true,
            now_ms: 2000,
        })
        .await?;
    if overwrite.record.change_id == first.record.change_id {
        return Err("overwrite did not mint a new change token".into());
    }
    package.instruction.text =
        InstructionText::try_from("Different shared instructions".to_owned())?;
    let changed = encode_schedule_package(&package, 1_048_576)?;
    match store
        .import_schedule::<String, String>(&ScheduleImport {
            operation_id: OperationId::generate(),
            package_utf8: &changed,
            overwrite: true,
            now_ms: 3000,
        })
        .await
    {
        Err(StorageError::InstructionImportConflict {
            instruction_id,
            schedule_ids,
        }) if instruction_id == package.instruction.instruction_id
            && schedule_ids == vec![package.schedule_id.clone()] => {}
        _ => {
            return Err(
                "instruction conflict omitted the exact shared document and schedules".into(),
            );
        }
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
