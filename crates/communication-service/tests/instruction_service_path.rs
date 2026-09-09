use automation_storage::AutomationStore;
use communication_client::ControlClient;
use communication_protocol::{
    InstructionCreateParams, InstructionShowParams, InstructionText, OperationId,
};
use communication_service::{ServiceIdentity, serve_control_connection};
use std::sync::Arc;

#[tokio::test]
async fn real_control_client_creates_and_reads_durable_instruction()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: real SQLite and Unix socket; no native app-server or model involved.
    let path = std::env::temp_dir().join(format!(
        "instruction-rpc-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(AutomationStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "instruction-test", "1").await?;
    let request = InstructionCreateParams {
        operation_id: OperationId::generate(),
        text: InstructionText::try_from("Check the workspace".to_owned())?,
    };
    // Act: create via public SDK, replay, and inspect through the same service.
    let first = client.create_instruction(request.clone()).await?;
    let replay = client.create_instruction(request).await?;
    let read = client
        .read_instruction(InstructionShowParams {
            instruction_id: first.instruction_id.clone(),
        })
        .await?;
    // Assert: actual RPC reaches durable storage and preserves operation identity.
    if first.instruction_id != replay.instruction_id || read.text.as_str() != "Check the workspace"
    {
        return Err("instruction service path did not preserve content/identity".into());
    }
    let update = communication_protocol::InstructionUpdateParams {
        operation_id: OperationId::generate(),
        instruction_id: first.instruction_id.clone(),
        expected_revision_id: first.revision_id.clone(),
        text: InstructionText::try_from("Updated instructions".to_owned())?,
    };
    let updated = client.update_instruction(update.clone()).await?;
    let stale = communication_protocol::InstructionUpdateParams {
        operation_id: OperationId::generate(),
        ..update
    };
    let error = client
        .update_instruction(stale)
        .await
        .err()
        .ok_or("stale update accepted")?;
    let communication_client::InstructionClientError::Rejected(failure) = error else {
        return Err("stale update lacked typed conflict".into());
    };
    let encoded = serde_json::to_value(failure)?;
    if encoded.get("currentRevisionId") != Some(&serde_json::to_value(updated.revision_id)?) {
        return Err(
            "revision conflict omitted the revision observed by the rejecting transaction".into(),
        );
    }
    client.close().await?;
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
