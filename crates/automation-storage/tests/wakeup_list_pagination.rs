use agent_automation::{DurableMessage, ExpiryRule, OperationId, TimingRule};
use automation_storage::{AutomationStore, WakeCreate};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct TestMessage {
    target: String,
    text: String,
}
impl DurableMessage for TestMessage {
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
async fn listing_keeps_membership_stable_when_new_wakes_arrive()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "wake-list-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    for now_ms in [0, 1000, 2000] {
        store
            .create_wakeup(&WakeCreate {
                operation_id: OperationId::generate(),
                message: TestMessage {
                    target: "B".into(),
                    text: "Check".into(),
                },
                timing: TimingRule::After { seconds: 60 },
                expiry: ExpiryRule::None,
                now_ms,
            })
            .await?;
    }
    let first = store.list_wakeups::<TestMessage>(None, 2).await?;
    if first.records.len() != 2 {
        return Err("first wake page missing records".into());
    }
    let cursor = first.next.ok_or("missing continuation")?;
    store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: TestMessage {
                target: "B".into(),
                text: "new".into(),
            },
            timing: TimingRule::After { seconds: 60 },
            expiry: ExpiryRule::None,
            now_ms: 3000,
        })
        .await?;
    let second = store.list_wakeups::<TestMessage>(Some(cursor), 2).await?;
    if second.records.len() != 1 || second.next.is_some() {
        return Err("new wake leaked into older listing membership".into());
    }
    let fresh = store.list_wakeups::<TestMessage>(None, 100).await?;
    if fresh.records.len() != 4 {
        return Err("fresh listing omitted new wake".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
