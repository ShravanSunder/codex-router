//! Real SDK/socket/SQLite package transfers work without a native backend.
use communication_client::ControlClient;
use communication_protocol::{
    InstructionCreateParams, InstructionText, OperationId, ScheduleCreateRequest,
    ScheduleImportRequest, ScheduleShowRequest,
};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sdk_exports_and_imports_disabled_schedule_with_replay()
-> Result<(), Box<dyn std::error::Error>> {
    exercise_package(false).await
}
#[tokio::test]
async fn sdk_fresh_mode_export_overwrite_and_local_setup_preserve_mode()
-> Result<(), Box<dyn std::error::Error>> {
    exercise_package(true).await
}
async fn exercise_package(fresh: bool) -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-package-{}.sqlite",
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
    let mut task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "package-test", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Check the build".to_owned())?,
        })
        .await?;
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"source-local"});
    let destination = if fresh {
        json!({"kind":"freshEachRun","endpoint":endpoint,"cwd":"/source-workspace"})
    } else {
        json!({"kind":"unprepared"})
    };
    let request: ScheduleCreateRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":600},"enabled":false,"destination":destination,"executionTimeoutSeconds":null}}),
    )?;
    let first = client.create_schedule(request).await?;
    let package = client
        .export_schedule(ScheduleShowRequest {
            schedule_id: first.schedule_id.clone(),
        })
        .await?;
    let import = ScheduleImportRequest {
        operation_id: OperationId::generate(),
        package_utf8: package.package_utf8,
        overwrite: true,
    };
    let mut imported = client.import_schedule(import.clone()).await?;
    let replay = client.import_schedule(import).await?;
    if imported.schedule_id != first.schedule_id
        || imported.change_id == first.change_id
        || imported.definition.enabled
        || serde_json::to_value(&imported)? != serde_json::to_value(replay)?
    {
        return Err(
            "package import lost identity, overwrite token, disabled state or replay outcome"
                .into(),
        );
    }
    if fresh {
        if !matches!(
            imported.definition.destination,
            communication_protocol::ExecutionDestination::FreshEachRunUnprepared
        ) {
            return Err("SDK import lost fresh mode or retained local binding".into());
        }
        let enable = client
            .enable_schedule(communication_protocol::ScheduleEnableRequest {
                operation_id: OperationId::generate(),
                schedule_id: imported.schedule_id.clone(),
            })
            .await;
        if !matches!(enable, Err(communication_client::ScheduleClientError::Rejected(ref error)) if error.field.as_deref() == Some("destination") && error.message.contains("schedule update"))
        {
            return Err("unprepared fresh schedule lacked actionable enable rejection".into());
        }
        client.close().await?;
        task.await??;
        let (socket, server) = tokio::net::UnixStream::pair()?;
        task = tokio::spawn(serve_control_connection(server, identity.clone()));
        client = ControlClient::initialize(socket, "fresh-local-setup", "1").await?;
        let mut definition = imported.definition.clone();
        definition.destination = communication_protocol::ExecutionDestination::FreshEachRun {
            endpoint: serde_json::from_value(
                json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"destination-local"}),
            )?,
            cwd: "/destination-workspace".into(),
        };
        imported = client
            .update_schedule(communication_protocol::ScheduleUpdateRequest {
                operation_id: OperationId::generate(),
                schedule_id: imported.schedule_id.clone(),
                expected_change_id: imported.change_id.clone(),
                definition,
            })
            .await?;
        if !matches!(imported.definition.destination, communication_protocol::ExecutionDestination::FreshEachRun { ref cwd, .. } if cwd == "/destination-workspace")
            || imported.definition.enabled
        {
            return Err("local setup changed mode or enabled execution".into());
        }
    }
    // Escaping may exceed the frame even when the source string itself is within its text limit.
    let oversized = client
        .import_schedule(ScheduleImportRequest {
            operation_id: OperationId::generate(),
            package_utf8: "\"".repeat(600_000),
            overwrite: true,
        })
        .await;
    match oversized {
        Err(communication_client::ScheduleClientError::Rejected(error))
            if matches!(error.details, communication_protocol::ScheduleFailureDetails::FrameLimit { encoded_bytes, maximum_bytes } if encoded_bytes > maximum_bytes)
                && error.field.as_deref() == Some("packageUtf8") => {}
        _ => return Err("oversized import did not return typed encoded-frame feedback".into()),
    }
    client
        .update_instruction(communication_protocol::InstructionUpdateParams {
            operation_id: OperationId::generate(),
            instruction_id: instruction.instruction_id,
            expected_revision_id: instruction.revision_id,
            text: InstructionText::try_from("\"".repeat(300_000))?,
        })
        .await?;
    let oversized = client
        .export_schedule(ScheduleShowRequest {
            schedule_id: first.schedule_id.clone(),
        })
        .await;
    match oversized {
        Err(communication_client::ScheduleClientError::Rejected(error)) if matches!(error.details, communication_protocol::ScheduleFailureDetails::FrameLimit { encoded_bytes, maximum_bytes } if encoded_bytes > maximum_bytes) =>
            {}
        _ => {
            return Err(
                "oversized export did not account for the escaped response envelope".into(),
            );
        }
    }
    // Local preflight above preserves the connection; server errors follow the existing retirement contract.
    client.close().await?;
    task.await??;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "package-reconnect", "1").await?;
    let inspected = client
        .read_schedule(ScheduleShowRequest {
            schedule_id: first.schedule_id,
        })
        .await?;
    if inspected.change_id != imported.change_id {
        return Err("package size rejection changed schedule state".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
