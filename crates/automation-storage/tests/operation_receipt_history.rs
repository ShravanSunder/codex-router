use agent_automation::{InstructionText, OperationId};
use automation_storage::{AutomationStore, InstructionUpdate, StoredOperationState};

#[tokio::test]
async fn operation_inspection_keeps_original_result_after_edits_and_event_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "operation-inspection-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let operation_id = OperationId::generate();
    let first = store
        .create_instruction(
            &operation_id,
            &InstructionText::try_from("Original instructions".to_owned())?,
            0,
        )
        .await?;
    store
        .update_instruction(&InstructionUpdate {
            operation_id: OperationId::generate(),
            instruction_id: first.instruction_id.clone(),
            expected_revision_id: first.revision_id,
            text: InstructionText::try_from("Current instructions".to_owned())?,
            now_ms: 1000,
        })
        .await?;
    let now = chrono::DateTime::parse_from_rfc3339("2026-08-31T12:00:00Z")?.timestamp_millis();
    store.prune_automation_events(now, 1000).await?;
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let record = store.read_operation(&operation_id).await?;
    if record.method != "instruction/create" || record.resource_id != first.instruction_id.as_str()
    {
        return Err("operation inspection changed its method/resource binding".into());
    }
    let StoredOperationState::Succeeded { result } = record.state else {
        return Err("committed operation lost its success receipt".into());
    };
    if result.get("text").and_then(serde_json::Value::as_str) != Some("Original instructions") {
        return Err(
            "operation inspection returned current text instead of its original result".into(),
        );
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
