use agent_automation::{DurableMessage, ExpiryRule, OperationId, TimingRule, WakeState};
use automation_storage::{AutomationStore, WakeAction, WakeCreate, WakeEvaluation, WakeMutation};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Message {
    target: String,
    text: String,
}
impl DurableMessage for Message {
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
async fn pause_discards_unsent_input_and_resume_keeps_original_timing()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: first reminder has fired but has not been dispatched.
    let path = std::env::temp_dir().join(format!(
        "wake-lifecycle-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: Message {
                target: "B".into(),
                text: "Check".into(),
            },
            timing: TimingRule::Interval { seconds: 60 },
            expiry: ExpiryRule::After { seconds: 600 },
            now_ms: 0,
        })
        .await?;
    let id = wake.definition.wakeup_id;
    let first = match store.evaluate_wakeup::<Message>(&id, 60000).await? {
        WakeEvaluation::Fired { fire, .. } => fire,
        _ => return Err("first firing absent".into()),
    };
    // Act: pause removes pending input; resume skips elapsed ticks without extending expiry.
    let paused = store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: id.clone(),
            action: WakeAction::Pause,
            now_ms: 70000,
        })
        .await?;
    if paused.wake.state != WakeState::Paused
        || paused.discarded.len() != 1
        || paused.wake.pending_delivery_id.is_some()
    {
        return Err("pause did not discard unsent reminder".into());
    }
    let resumed = store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: id.clone(),
            action: WakeAction::Resume,
            now_ms: 190000,
        })
        .await?;
    // Assert: next original tick 240s, original expiry 600s, first-fire history preserved.
    if resumed.wake.state != WakeState::Active
        || resumed.wake.next_due_at_ms != Some(240000)
        || resumed.wake.definition.expires_at_ms != Some(600000)
        || resumed.wake.first_fire != Some(first)
    {
        return Err("resume changed anchor/expiry/history".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
#[tokio::test]
async fn paused_one_shot_finishes_without_firing_after_its_due_time()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: one-shot paused before its only due instant.
    let path = std::env::temp_dir().join(format!(
        "wake-one-shot-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: Message {
                target: "B".into(),
                text: "Check".into(),
            },
            timing: TimingRule::After { seconds: 60 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let id = wake.definition.wakeup_id;
    store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: id.clone(),
            action: WakeAction::Pause,
            now_ms: 30000,
        })
        .await?;
    // Act / Assert: no replay on resume and no invented firing to satisfy a waiter.
    let resumed = store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: id,
            action: WakeAction::Resume,
            now_ms: 120000,
        })
        .await?;
    if resumed.wake.state != WakeState::Finished
        || resumed.wake.first_fire.is_some()
        || resumed.wake.next_due_at_ms.is_some()
    {
        return Err("exhausted one-shot did not finish honestly".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
