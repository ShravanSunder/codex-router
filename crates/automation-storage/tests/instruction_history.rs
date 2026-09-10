use agent_automation::{InstructionText, OperationId};
use automation_storage::{AutomationStore, InstructionUpdate};
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn edit_preserves_history_and_stale_edit_cannot_replace_current_text()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: one current document and two callers holding the same revision.
    let path = std::env::temp_dir().join(format!(
        "automation-history-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let initial = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Original task".to_owned())?,
            100,
        )
        .await?;
    let update = InstructionUpdate {
        operation_id: OperationId::generate(),
        instruction_id: initial.instruction_id.clone(),
        expected_revision_id: initial.revision_id.clone(),
        text: InstructionText::try_from("Updated task".to_owned())?,
        now_ms: 200,
    };
    // Act: update once, replay that request, then try a stale competing edit.
    let updated = store.update_instruction(&update).await?;
    let replayed = store.update_instruction(&update).await?;
    let stale = InstructionUpdate {
        operation_id: OperationId::generate(),
        text: InstructionText::try_from("Stale task".to_owned())?,
        ..update
    };
    if store.update_instruction(&stale).await.is_ok() {
        return Err("stale update overwrote current revision".into());
    }
    let current = store.read_instruction(&initial.instruction_id).await?;
    // Assert: current is new, replay stable, original creation time and history retained.
    if updated != replayed
        || current != updated
        || current.revision_id == initial.revision_id
        || current.created_at_ms != 100
        || current.text.as_str() != "Updated task"
    {
        return Err("instruction revision/current/replay contract broken".into());
    }
    store.close().await?;
    let mut connection =
        SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path))
            .await?;
    let history: Vec<String> = sqlx::query_scalar(
        "SELECT instruction_text FROM instruction_revisions ORDER BY recorded_at_ms",
    )
    .fetch_all(&mut connection)
    .await?;
    if history != ["Original task", "Updated task"] {
        return Err("revision history lost or duplicated".into());
    }
    connection.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
