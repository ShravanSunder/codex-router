use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::ControlClient;
use communication_protocol::{InstructionCreateParams, InstructionText, OperationId};
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn host_supplies_automation_storage_without_native_backend()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a private, short test socket directory; no native process is launched.
    let root = std::path::PathBuf::from(format!(
        "/tmp/host-automation-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("absent-native.sock"),
        native_schema: None,
    })
    .await?;
    let mut client = ControlClient::connect(&root, "automation-composition", "1").await?;
    // Act: Host-created service handles a real SDK request with no native endpoint.
    let document = client
        .create_instruction(InstructionCreateParams {
            operation_id: OperationId::generate(),
            text: InstructionText::try_from("Check the repository".to_owned())?,
        })
        .await?;
    // Assert: separate automation database exists and contains the requested current text.
    if document.text.as_str() != "Check the repository"
        || !root.join("automation.sqlite").exists()
        || !root.join("session-registry.sqlite").exists()
    {
        return Err("Host did not compose separate automation persistence".into());
    }
    client.close().await?;
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected runtime cleanup entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
