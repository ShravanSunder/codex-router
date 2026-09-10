use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::ControlClient;
use communication_protocol::{OperationId, WakeSendRequest, WakeShowRequest};
use serde_json::json;
use std::{os::unix::fs::DirBuilderExt, time::Duration};

#[tokio::test]
async fn host_records_firing_without_native_backend_acceptance()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "host-wake-timer-{}",
        OperationId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("unavailable-native.sock"),
        native_schema: None,
    })
    .await?;
    let mut client = ControlClient::connect(&root, "wake-timer-test", "1").await?;
    let request: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":{"target":{"endpoint":{"serviceId":runtime.service_id(),"endpointId":"codex-local"},"sessionId":"fixture-only-new-thread"},"content":{"kind":"humanUser","text":"Check"},"delivery":"auto","generationGuard":null},"timing":{"kind":"at","at":"2026-01-01T00:00:00.000Z"},"expiry":{"kind":"none"}}),
    )?;
    let created = client.send_wakeup(request).await?;
    let id = created.definition.wakeup_id;
    let observed = tokio::time::timeout(Duration::from_secs(3), async {
        let mut interval = tokio::time::interval(Duration::from_millis(20));
        loop {
            interval.tick().await;
            let current = client
                .read_wakeup(WakeShowRequest {
                    wakeup_id: id.clone(),
                })
                .await?;
            if current.first_fire.is_some() {
                return Ok::<_, communication_client::WakeClientError>(current);
            }
        }
    })
    .await??;
    if observed.pending_delivery_id.is_none() {
        return Err("firing lost durable delivery identity".into());
    }
    client.close().await?;
    runtime.shutdown().await?;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err("unexpected owned fixture entry".into());
        }
        std::fs::remove_file(entry.path())?;
    }
    std::fs::remove_dir(root)?;
    Ok(())
}
