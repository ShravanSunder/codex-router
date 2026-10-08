use collaboration_client::WakeWaitError;
use collaboration_protocol::{OperationId, WakeMutationRequest, WakeSendRequest, WakeShowRequest};
use collaboration_service::ServiceIdentity;
use serde_json::json;
use std::sync::Arc;
#[path = "support/served_api.rs"]
mod served_api;
#[path = "support/wake_push_draft.rs"]
mod wake_push_test_support;
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
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(store.clone());
    let served = served_api::ServedApi::start(identity).await?;
    let client = served.client("wake-control").await?;
    let request: WakeSendRequest = serde_json::from_value(
        json!({"operationId":OperationId::generate(),"message":{"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture-only"},"content":{"kind":"humanUser","text":"Check"},"delivery":"auto","generationGuard":null},"timing":{"kind":"interval","seconds":60},"expiry":{"kind":"none"}}),
    )?;
    let wake = client.send_wakeup(request).await?;
    let wait = served
        .client("wake-wait")
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
        .evaluate_wakeup::<collaboration_protocol::SavedMessage>(
            &wake.definition.wakeup_id,
            chrono::Utc::now().timestamp_millis() + 60000,
            wake_push_test_support::build_test_wake_push_draft,
        )
        .await?;
    client
        .cancel_wakeup(WakeMutationRequest {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    let historical = served
        .client("wake-history")
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
    let missing = served
        .client("wake-missing")
        .await?
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: collaboration_protocol::WakeupId::generate(),
        })
        .await;
    if !matches!(missing, Err(WakeWaitError::NotFound { .. })) {
        return Err("missing wake was confused with unavailable observation".into());
    }
    served.stop().await?;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
