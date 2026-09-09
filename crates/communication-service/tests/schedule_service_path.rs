//! Real local SDK/socket/storage path; no native runtime or model.
use communication_client::ControlClient;
use communication_protocol::{
    InstructionCreateParams, InstructionText, OperationId, ScheduleCreateRequest,
    ScheduleShowRequest,
};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;
#[tokio::test]
async fn sdk_creates_disabled_schedule_without_native_backend()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-service-{}.sqlite",
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
    let mut client = ControlClient::initialize(socket, "schedule-test", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Inspect build status".to_owned())?,
        })
        .await?;
    let request: ScheduleCreateRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":600},"enabled":false,"destination":{"kind":"unprepared"},"executionTimeoutSeconds":null}}),
    )?;
    let first = client.create_schedule(request.clone()).await?;
    let replay = client.create_schedule(request).await?;
    if serde_json::to_value(&first)? != serde_json::to_value(replay)? {
        return Err("schedule creation replay changed result".into());
    }
    let read = client
        .read_schedule(ScheduleShowRequest {
            schedule_id: first.schedule_id.clone(),
        })
        .await?;
    if read.definition.enabled || read.active_run_id.is_some() || read.waiting_run_id.is_some() {
        return Err("new disabled schedule invented running work".into());
    }
    let rejected = client
        .enable_schedule(communication_protocol::ScheduleEnableRequest {
            operation_id: OperationId::generate(),
            schedule_id: first.schedule_id,
        })
        .await;
    if !matches!(
        rejected,
        Err(communication_client::ScheduleClientError::Rejected(_))
    ) {
        return Err("unprepared schedule enabled or failed without typed feedback".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
