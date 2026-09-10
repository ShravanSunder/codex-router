//! A paused one-shot cannot fabricate a first firing when resumed after its due time.
use automation_storage::{AutomationStore, WakeAction, WakeCreate, WakeMutation};
use communication_client::{ControlClient, WakeWaitError};
use communication_protocol::{OperationId, SavedMessage, WakeShowRequest};
use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn sdk_wait_reports_skipped_one_shot_without_a_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "skipped-wait-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let message: SavedMessage = serde_json::from_value(json!({
        "target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"fixture"},
        "content":{"kind":"humanUser","text":"Reminder"},"delivery":"auto","generationGuard":null
    }))?;
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message,
            timing: agent_automation::TimingRule::After { seconds: 1 },
            expiry: agent_automation::ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    for (action, now_ms) in [(WakeAction::Pause, 500), (WakeAction::Resume, 2000)] {
        store
            .mutate_wakeup::<SavedMessage>(&WakeMutation {
                operation_id: OperationId::generate(),
                wakeup_id: wake.definition.wakeup_id.clone(),
                action,
                now_ms,
            })
            .await?;
    }
    let store = Arc::new(tokio::sync::Mutex::new(store));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_automation_store(Arc::clone(&store));
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "skipped-wait", "1").await?;
    let current = client
        .read_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id.clone(),
        })
        .await?;
    if current.first_fire.is_some() || current.pending_delivery_id.is_some() {
        return Err("skipped wake fabricated firing or delivery".into());
    }
    let result = client
        .subscribe_wakeup(WakeShowRequest {
            wakeup_id: wake.definition.wakeup_id,
        })
        .await?
        .wait_until_first_fire()
        .await;
    if !matches!(result, Err(WakeWaitError::FinishedWithoutFiring { .. })) {
        return Err("skipped one-shot did not return its typed terminal wait error".into());
    }
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
