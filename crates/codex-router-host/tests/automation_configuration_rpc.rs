//! Host configuration is changed through the real SDK, then verified after Host restart.
use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::ControlClient;
use communication_protocol::{AutomationConfigureRequest, OperationId};
use std::os::unix::fs::DirBuilderExt;
#[tokio::test]
async fn host_persists_configured_budgets_and_loads_them_on_restart()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "automation-config-rpc-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    for first in [true, false] {
        let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
            directory: root.clone(),
            codex_home: root.clone(),
            backend_socket: root.join("absent.sock"),
            native_schema: None,
        })
        .await?;
        let mut client = ControlClient::connect(&root, "configuration-rpc-test", "1").await?;
        if first {
            client
                .configure_automation(AutomationConfigureRequest {
                    operation_id: OperationId::generate(),
                    execution_timeout_seconds: 120.try_into()?,
                    summary_timeout_seconds: 30.try_into()?,
                })
                .await?;
        }
        let status = client.automation_status().await?;
        let configured = status.configuration.ok_or("configuration not ready")?;
        if u32::from(configured.execution_timeout_seconds) != 120
            || u32::from(configured.summary_timeout_seconds) != 30
        {
            return Err("configured budgets lost across restart".into());
        }
        client.close().await?;
        runtime.shutdown().await?;
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected fixture entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
