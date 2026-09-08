use agent_automation::{DurableMessage, ExpiryRule, OperationId, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
struct ReminderMessage {
    target: String,
    text: String,
}
impl DurableMessage for ReminderMessage {
    type Target = String;
    type Content = String;
    type Generation = String;
    fn target(&self) -> &String {
        &self.target
    }
    fn content(&self) -> &String {
        &self.text
    }
    fn generation_guard(&self) -> Option<&String> {
        None
    }
    fn delivery_mode(&self) -> &'static str {
        "auto"
    }
}
#[tokio::test]
async fn worker_inventory_selects_due_wakes_and_eligible_deliveries_only()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wake-worker-inventory-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: ReminderMessage {
                target: "B".into(),
                text: "Check".into(),
            },
            timing: TimingRule::Interval { seconds: 60 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    if !store.due_wakeup_ids(59999, 100).await?.is_empty() {
        return Err("wake was selected before due time".into());
    }
    if store.due_wakeup_ids(60000, 100).await? != vec![wake.definition.wakeup_id.clone()] {
        return Err("due wake was not selected".into());
    }
    let id = match store
        .evaluate_wakeup::<ReminderMessage>(&wake.definition.wakeup_id, 60000)
        .await?
    {
        WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => return Err("missing fire".into()),
    };
    if !store.due_wakeup_ids(60000, 100).await?.is_empty() {
        return Err("handled timing boundary remained due".into());
    }
    if store.eligible_delivery_ids(60000, 100).await? != vec![id.clone()] {
        return Err("eligible delivery missing".into());
    }
    let _claim = store
        .claim_delivery::<String, String, String>(&id, 60000)
        .await?
        .ok_or("missing claim")?;
    if !store.eligible_delivery_ids(100000, 100).await?.is_empty() {
        return Err("in-flight dispatch selected for replay".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
