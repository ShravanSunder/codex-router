use communication_client::ControlClient;
use communication_protocol::{
    InstructionCreateParams, InstructionText, OperationId, ScheduleCreateRequest,
};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;
#[tokio::test]
async fn enabled_schedule_requires_available_native_capabilities()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "schedule-admission-{}.sqlite",
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
    let mut client = ControlClient::initialize(socket, "schedule-activation", "1").await?;
    let instruction = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Check".to_owned())?,
        })
        .await?;
    let request: ScheduleCreateRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"definition":{"instructionId":instruction.instruction_id,"timing":{"kind":"interval","seconds":60},"enabled":true,"destination":{"kind":"freshEachRun","endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"cwd":"/fixture"},"executionTimeoutSeconds":null}}),
    )?;
    if client.create_schedule(request).await.is_ok() {
        return Err("enabled schedule admitted without native execution capabilities".into());
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
