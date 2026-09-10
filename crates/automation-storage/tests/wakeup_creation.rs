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
async fn wake_creation_survives_reopen_and_replay_preserves_original_timing()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: a captured message and relative timing/expiry.
    let path = std::env::temp_dir().join(format!(
        "automation-wake-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let mut request = WakeCreate {
        operation_id: OperationId::generate(),
        message: TestMessage {
            target: "B".into(),
            text: "Check job".into(),
        },
        timing: TimingRule::Interval { seconds: 600 },
        expiry: ExpiryRule::After { seconds: 7200 },
        now_ms: 1000,
    };
    // Act: create, reopen, then replay with a later current time.
    let first = store.create_wakeup(&request).await?;
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    request.now_ms = 9000;
    let replay = store.create_wakeup(&request).await?;
    let current = store
        .read_wakeup::<TestMessage>(&first.definition.wakeup_id)
        .await?;
    // Assert: saved message and anchor/expiry remain unchanged; no firing claimed.
    if first != replay
        || current != first
        || first.next_due_at_ms != Some(601000)
        || first.definition.expires_at_ms != Some(7201000)
        || first.first_fire.is_some()
    {
        return Err("wake creation/replay changed timing or claimed firing".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
