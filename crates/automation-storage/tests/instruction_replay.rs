use agent_automation::{InstructionText, OperationId};
use automation_storage::AutomationStore;

#[tokio::test]
async fn lost_creation_response_replays_the_original_instruction_after_reopen()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: stable caller operation ID, text and an isolated database.
    let path = std::env::temp_dir().join(format!(
        "automation-instruction-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let operation = OperationId::generate();
    let text = InstructionText::try_from("Check repository status".to_owned())?;
    let mut store = AutomationStore::open(&path).await?;
    // Act: repeat after reconnect, with a later current clock.
    let original = store.create_instruction(&operation, &text, 100).await?;
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let replayed = store.create_instruction(&operation, &text, 200).await?;
    // Assert: no new instruction, revision or creation timestamp is minted on replay.
    if original != replayed {
        return Err("replay created a different instruction".into());
    }
    let different = InstructionText::try_from("Different work".to_owned())?;
    if store
        .create_instruction(&operation, &different, 300)
        .await
        .is_ok()
    {
        return Err("conflicting operation payload was accepted".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
