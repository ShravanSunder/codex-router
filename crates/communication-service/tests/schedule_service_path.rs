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

#[tokio::test]
async fn sdk_rejects_mode_change_and_thread_preparation_with_actionable_feedback()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-mode-sdk-{}.sqlite",
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
    let service = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "mode-fixture", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: "Check".to_owned().try_into()?,
        })
        .await?;
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
    let created = client.create_schedule(serde_json::from_value(json!({
        "operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,
        "timing":{"kind":"interval","seconds":60},"enabled":false,
        "destination":{"kind":"freshEachRun","endpoint":endpoint,"cwd":"/isolated-test"},"executionTimeoutSeconds":null}
    }))?).await?;
    client.close().await?;
    service.await??;
    for prepare in [false, true] {
        let (socket, server) = tokio::net::UnixStream::pair()?;
        let service = tokio::spawn(serve_control_connection(server, identity.clone()));
        let mut client = ControlClient::initialize(socket, "mode-rejection", "1").await?;
        let result = if prepare {
            client
                .prepare_schedule(serde_json::from_value(json!({
                    "operationId":OperationId::generate(),"scheduleId":created.schedule_id,
                    "destination":{"kind":"fresh","endpoint":endpoint,"cwd":"/isolated-test"}
                }))?)
                .await
        } else {
            let mut definition = created.definition.clone();
            definition.destination = communication_protocol::ExecutionDestination::Unprepared;
            client
                .update_schedule(communication_protocol::ScheduleUpdateRequest {
                    operation_id: OperationId::generate(),
                    schedule_id: created.schedule_id.clone(),
                    expected_change_id: created.change_id.clone(),
                    definition,
                })
                .await
        };
        let Err(communication_client::ScheduleClientError::Rejected(error)) = result else {
            return Err("mode change lacked typed rejection".into());
        };
        if !matches!(
            error.kind,
            communication_protocol::ScheduleFailureKind::InvalidField
        ) || error.field.as_deref() != Some("destination")
            || !error.message.contains("fixed at creation")
            || !matches!(
                error.next_action,
                communication_protocol::ScheduleNextAction::CorrectRequest
            )
        {
            return Err("mode rejection omitted field, reason or recovery action".into());
        }
        client.close().await?;
        service.await??;
    }
    let current = store.lock().await.inspect_schedule::<communication_protocol::SessionRef, communication_protocol::EndpointRef>(&created.schedule_id).await?;
    if current.record.change_id != created.change_id
        || current.active_run_id.is_some()
        || current.waiting_run_id.is_some()
    {
        return Err("rejected mode request changed schedule or started work".into());
    }
    drop(identity);
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
