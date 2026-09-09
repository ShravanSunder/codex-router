use communication_client::{ControlClient, WakeWaitError};
use communication_protocol::{OperationId, WakeMutationRequest, WakeSendRequest, WakeShowRequest};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;
#[tokio::test]
async fn dedicated_wait_observes_pause_after_immediate_resume()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wake-first-fire-{}.sqlite",
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
    let task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(socket, "wake-control", "1").await?;
    let request: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":{"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture-only"},"content":{"kind":"humanUser","text":"Check"},"delivery":"auto","generationGuard":null},"timing":{"kind":"interval","seconds":60},"expiry":{"kind":"none"}}),
    )?;
    let wake = client.send_wakeup(request).await?;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let waiter_task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let wait = ControlClient::initialize(socket, "wake-wait", "1")
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    client
        .pause_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    client
        .resume_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        wait.wait_until_first_fire(),
    )
    .await?;
    if !matches!(outcome, Err(WakeWaitError::Paused { .. })) {
        return Err("wait missed pause during rapid resume".into());
    }
    store
        .lock()
        .await
        .evaluate_wakeup::<communication_protocol::SavedMessage>(
            &wake.definition.wakeup_id,
            chrono::Utc::now().timestamp_millis() + 60000,
        )
        .await?;
    client
        .cancel_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let history_task = tokio::spawn(serve_control_connection(server, identity.clone()));
    let historical = ControlClient::initialize(socket, "wake-history", "1")
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?
        .wait_until_first_fire()
        .await?;
    if historical.wakeup_id != wake.definition.wakeup_id {
        return Err("historical firing identity changed".into());
    }
    history_task.await??;
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let missing_task = tokio::spawn(serve_control_connection(server, identity));
    let missing = ControlClient::initialize(socket, "wake-missing", "1")
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: communication_protocol::WakeupId::generate(),
        })
        .await;
    if !matches!(missing, Err(WakeWaitError::NotFound { .. })) {
        return Err("missing wake was confused with unavailable observation".into());
    }
    missing_task.await??;
    client.close().await?;
    task.await??;
    waiter_task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
