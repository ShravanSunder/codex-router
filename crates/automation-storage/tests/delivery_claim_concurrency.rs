use agent_automation::{DurableMessage, ExpiryRule, OperationId, TimingRule};
use automation_storage::{AutomationStore, WakeCreate, WakeEvaluation};
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
async fn two_workers_claim_one_delivery_and_reopen_never_reclaims_dispatching()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: one fired reminder, two independent SQLite worker connections.
    let path = std::env::temp_dir().join(format!(
        "delivery-claim-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut first = AutomationStore::open(&path).await?;
    let wake = first
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: Message {
                target: "B".into(),
                text: "Check job".into(),
            },
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let id = match first
        .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 1000)
        .await?
    {
        WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => return Err("reminder did not fire".into()),
    };
    let mut second = AutomationStore::open(&path).await?;
    // Act: race claims; process loss after claim must not enable another claim on reopen.
    let (left, right) = tokio::join!(
        first.claim_delivery::<String, String, String>(&id, 1000),
        second.claim_delivery::<String, String, String>(&id, 1000)
    );
    let claimed = [left?, right?].into_iter().flatten().collect::<Vec<_>>();
    if claimed.len() != 1 {
        return Err("delivery claim was lost or duplicated".into());
    }
    let claim = claimed.into_iter().next().ok_or("claim missing")?;
    if claim.content != "Check job" || claim.target != "B" {
        return Err("claimed payload changed".into());
    }
    first.close().await?;
    second.close().await?;
    let mut reopened = AutomationStore::open(&path).await?;
    if reopened
        .claim_delivery::<String, String, String>(&id, 2000)
        .await?
        .is_some()
    {
        return Err("reopen replayed a possibly submitted message".into());
    }
    reopened.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
