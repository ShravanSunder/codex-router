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
async fn unknown_resume_effect_is_retained_even_when_input_was_not_dispatched()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "delivery-partial-{}.sqlite",
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
    let mut evidence = effects(SubmissionEffect::NotDispatched);
    evidence.resume = PreparationEffect::Unknown;
    store
        .complete_delivery(DeliveryCompletion::<_, _, String> {
            delivery_id: id.clone(),
            attempt_id: claim.attempt_id,
            effects: evidence,
            result: DeliveryResult::Unknown {
                reason: "resume response lost before input submission".into(),
            },
            now_ms: 2000,
        })
        .await?;
    if store
        .claim_delivery::<String, String, String>(&id, 100000)
        .await?
        .is_some()
    {
        return Err("unknown resume was silently replayed".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
