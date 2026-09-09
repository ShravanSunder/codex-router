//! Bounded SDK discovery through actual service sockets and SQLite, with no native backend.
use communication_client::ControlClient;
use communication_protocol::{
    AutomationPageRequest, DeliveryListRequest, InstructionCreateParams, InstructionText,
    OperationId, RevisionListRequest, RunListRequest, ScheduleCreateRequest,
};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sdk_lists_local_collections_and_rejects_cross_collection_cursor()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "collection-service-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&path).await?,
    ));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "collection-test", "1").await?;
    let first = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("First task".to_owned())?,
        })
        .await?;
    client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Second task".to_owned())?,
        })
        .await?;
    let page = client
        .list_instructions(AutomationPageRequest {
            cursor: None,
            limit: 1.try_into()?,
        })
        .await?;
    if page.records.len() != 1 || page.records[0].instruction_id != first.instruction_id {
        return Err("instruction listing order incorrect".into());
    }
    let cursor = page.next_cursor.ok_or("missing next instruction cursor")?;
    let revisions = client
        .list_instruction_revisions(RevisionListRequest {
            instruction_id: first.instruction_id.clone(),
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    if revisions.records.len() != 1 || revisions.records[0].revision_id != first.revision_id {
        return Err("revision listing identity incorrect".into());
    }
    let request: ScheduleCreateRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"definition":{"instructionId":first.instruction_id,"timing":{"kind":"interval","seconds":600},"enabled":false,"destination":{"kind":"unprepared"},"executionTimeoutSeconds":null}}),
    )?;
    let schedule = client.create_schedule(request).await?;
    let schedules = client
        .list_schedules(AutomationPageRequest {
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    if schedules.records.len() != 1 || schedules.records[0].schedule_id != schedule.schedule_id {
        return Err("schedule listing identity incorrect".into());
    }
    let runs = client
        .list_runs(RunListRequest {
            schedule_id: schedule.schedule_id,
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    let deliveries = client
        .list_deliveries(DeliveryListRequest {
            wakeup_id: None,
            cursor: None,
            limit: 50.try_into()?,
        })
        .await?;
    if !runs.records.is_empty() || !deliveries.records.is_empty() {
        return Err("listing invented native or timed work".into());
    }
    let history = client
        .read_automation_events(communication_protocol::AutomationEventsRequest {
            after: None,
            limit: 50.try_into()?,
        })
        .await?;
    if history.records.len() != 3
        || !history.records.iter().any(|event| {
            matches!(
                event.details,
                communication_protocol::AutomationEventDetails::ScheduleEdit { .. }
            )
        })
    {
        return Err("event history omitted typed schedule definition evidence".into());
    }
    let continued = client
        .read_automation_events(communication_protocol::AutomationEventsRequest {
            after: Some(history.next_cursor),
            limit: 50.try_into()?,
        })
        .await?;
    if !continued.records.is_empty() {
        return Err("event cursor replayed consumed records".into());
    }
    let rejected = client
        .list_schedules(AutomationPageRequest {
            cursor: Some(cursor),
            limit: 50.try_into()?,
        })
        .await;
    if !matches!(rejected, Err(communication_client::AutomationInspectionClientError::Rejected(error)) if matches!(error.kind, communication_protocol::AutomationInspectionFailureKind::InvalidField) && error.field.as_deref() == Some("cursor"))
    {
        return Err("cross-collection cursor lacked typed rejection".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
