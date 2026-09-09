//! Export must reserve import-envelope space, not just fit its own response.
use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{EndpointRef, OperationId, ScheduleShowRequest, SessionRef};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn export_rejects_package_that_fits_response_but_not_import()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "package-boundary-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let instruction = store
        .create_instruction(&OperationId::generate(), &"x".to_owned().try_into()?, 0)
        .await?;
    let schedule = store
        .create_schedule(
            &automation_storage::ScheduleCreate::<SessionRef, EndpointRef> {
                operation_id: OperationId::generate(),
                definition: agent_automation::ScheduleDefinition {
                    instruction_id: instruction.instruction_id.clone(),
                    timing: agent_automation::TimingRule::Interval { seconds: 60 },
                    enabled: false,
                    destination: agent_automation::ExecutionDestination::Unprepared,
                    execution_timeout_seconds: None,
                },
                imported_continuity: agent_automation::ContinuityInput::None,
                now_ms: 0,
            },
        )
        .await?;
    let package = store
        .export_schedule::<SessionRef, EndpointRef>(&schedule.schedule_id)
        .await?;
    let encoded = agent_automation::encode_schedule_package(&package, usize::MAX)?;
    let response_size =
        serde_json::to_vec(&json!({"jsonrpc":"2.0","id":"2","result":{"packageUtf8":encoded}}))?
            .len();
    let text_size = communication_protocol::MAX_CONTROL_FRAME_BYTES - response_size - 16 + 1;
    store
        .update_instruction(&automation_storage::InstructionUpdate {
            operation_id: OperationId::generate(),
            instruction_id: instruction.instruction_id,
            expected_revision_id: instruction.revision_id,
            text: "x".repeat(text_size).try_into()?,
            now_ms: 1,
        })
        .await?;
    let store = Arc::new(tokio::sync::Mutex::new(store));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&store));
    let (client, server) = tokio::net::UnixStream::pair()?;
    let server = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(client, "roundtrip-boundary", "1").await?;
    let result = client
        .export_schedule(ScheduleShowRequest {
            schedule_id: schedule.schedule_id,
        })
        .await;
    let rejected = matches!(result, Err(communication_client::ScheduleClientError::Rejected(error)) if error.field.as_deref() == Some("packageUtf8"));
    client.close().await?;
    server.await??;
    drop(store);
    std::fs::remove_file(path)?;
    if !rejected {
        return Err(
            "export admitted a package without reserving its larger import envelope".into(),
        );
    }
    Ok(())
}
