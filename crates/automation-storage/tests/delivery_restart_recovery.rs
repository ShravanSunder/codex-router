use agent_automation::{
    AcceptedDeliveryEffect, CessationEvidence, DurableMessage, ExpiryRule, NativeEffectEvidence,
    OperationId, PreparationEffect, SubmissionEffect, TimingRule,
};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryPreparation, DeliveryResult, WakeCreate,
    WakeEvaluation,
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
    store
        .prepare_delivery(DeliveryPreparation {
            delivery_id: id.clone(),
            attempt_id: claim.attempt_id.clone(),
            effects: effects(SubmissionEffect::Dispatching).into(),
        })
        .await?;
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
            effects: Some(effects(SubmissionEffect::Accepted).into()),
            result: DeliveryResult::Accepted {
                effect: AcceptedDeliveryEffect::StartedOrSteered,
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

#[tokio::test]
async fn unselected_claim_recovers_as_known_not_submitted_and_retries()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "unselected-delivery-recovery-{}.sqlite",
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
    let first = store
        .claim_delivery::<String, String, String>(&id, 1000)
        .await?
        .ok_or("missing first claim")?;
    let claimed = store.read_delivery::<String, String, String>(&id).await?;
    if claimed
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.effects.as_ref())
        .is_some()
    {
        return Err("claim selected a route before dispatch".into());
    }
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    store.recover_interrupted_deliveries(2000).await?;
    let recovered = store.read_delivery::<String, String, String>(&id).await?;
    if recovered.status != agent_automation::DeliveryStatus::Retryable
        || !matches!(
            recovered.attempt.as_ref().map(|attempt| &attempt.outcome),
            Some(agent_automation::AttemptOutcome::KnownNotSubmitted {
                retryable: true,
                ..
            })
        )
    {
        return Err("pre-selection crash was treated as an uncertain submission".into());
    }
    let next = store
        .claim_delivery::<String, String, String>(&id, 100_000)
        .await?
        .ok_or("known-not-submitted attempt was not retried")?;
    if next.attempt_id == first.attempt_id {
        return Err("retry reused an attempt identity".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn paused_unselected_claim_is_discarded_after_recovery()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "paused-unselected-recovery-{}.sqlite",
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
    store
        .claim_delivery::<String, String, String>(&id, 1000)
        .await?
        .ok_or("missing claim")?;
    store
        .mutate_wakeup::<Message>(&automation_storage::WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id,
            action: automation_storage::WakeAction::Pause,
            now_ms: 1500,
        })
        .await?;
    store.close().await?;
    let mut store = AutomationStore::open(&path).await?;
    store.recover_interrupted_deliveries(2000).await?;
    let recovered = store.read_delivery::<String, String, String>(&id).await?;
    if recovered.status != agent_automation::DeliveryStatus::Discarded
        || store
            .claim_delivery::<String, String, String>(&id, 100_000)
            .await?
            .is_some()
    {
        return Err("pause resurrected a preselection attempt".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}
