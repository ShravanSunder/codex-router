use agent_automation::{
    CessationEvidence, DurableMessage, ExpiryRule, NativeEffectEvidence, OperationId,
    PreparationEffect, SubmissionEffect, TimingRule,
};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryResult, WakeCreate, WakeEvaluation,
};
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
fn effects(submission: SubmissionEffect) -> NativeEffectEvidence<String, String> {
    NativeEffectEvidence {
        target: Some("B".into()),
        generation: Some("g1".into()),
        client_user_message_id: None,
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::Accepted,
        submission,
        cessation: CessationEvidence::NotApplicable,
    }
}
#[tokio::test]
async fn explicit_host_recovery_preserves_identity_without_replaying_dispatch()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "delivery-recovery-{}.sqlite",
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
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let id = match store
        .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 1000)
        .await?
    {
        WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => return Err("missing fire".into()),
    };
    let claim = store
        .claim_delivery::<String, String, String>(&id, 1000)
        .await?
        .ok_or("missing claim")?;
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    let recovered = store.recover_interrupted_deliveries(2000).await?;
    if recovered != vec![id.clone()] {
        return Err("interrupted delivery was not recovered as uncertain".into());
    }
    if !store.recover_interrupted_deliveries(3000).await?.is_empty() {
        return Err("recovery repeated a resolved startup transition".into());
    }
    if store
        .claim_delivery::<String, String, String>(&id, 100000)
        .await?
        .is_some()
    {
        return Err("startup replayed interrupted dispatch".into());
    }
    // A supported reconciliation can still record actual evidence for that same attempt.
    if !store
        .complete_delivery(DeliveryCompletion {
            delivery_id: id,
            attempt_id: claim.attempt_id,
            effects: effects(SubmissionEffect::Accepted),
            result: DeliveryResult::Accepted {
                receipt: "verified native receipt".to_owned(),
            },
            now_ms: 100000,
        })
        .await?
    {
        return Err("recovery lost attempt correlation".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
