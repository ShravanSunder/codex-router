use agent_automation::{InstructionText, OperationId};
use automation_storage::{AutomationCollection, AutomationListPosition, AutomationStore};

#[tokio::test]
async fn instruction_pages_preserve_upper_bound_across_new_records()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "automation-pages-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let first = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("First instruction".to_owned())?,
            1000,
        )
        .await?;
    let second = store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Second instruction".to_owned())?,
            2000,
        )
        .await?;
    let page = store
        .list_collection_keys(&AutomationCollection::Instructions, None, 1)
        .await?;
    if page.records.len() != 1
        || page.records[0].resource_id != first.instruction_id.as_str()
        || !page.has_more
    {
        return Err("first page ordering/more boundary failed".into());
    }
    let position = AutomationListPosition {
        upper: page.upper.ok_or("upper key missing")?,
        last: page.records[0].clone(),
    };
    store
        .create_instruction(
            &OperationId::generate(),
            &InstructionText::try_from("Later instruction".to_owned())?,
            3000,
        )
        .await?;
    let page = store
        .list_collection_keys(&AutomationCollection::Instructions, Some(position), 1)
        .await?;
    if page.records.len() != 1
        || page.records[0].resource_id != second.instruction_id.as_str()
        || page.has_more
    {
        return Err("continued page included a record after its captured upper bound".into());
    }
    let revisions = store
        .list_collection_keys(
            &AutomationCollection::Revisions(first.instruction_id),
            None,
            50,
        )
        .await?;
    if revisions.records.len() != 1
        || revisions.records[0].resource_id != first.revision_id.as_str()
    {
        return Err("instruction revision filter returned another identity".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
