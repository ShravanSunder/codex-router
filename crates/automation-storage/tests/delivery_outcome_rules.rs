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
async fn known_nonsubmission_retries_but_uncertainty_never_does()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: one fired reminder and its claimed attempt.
    let path = std::env::temp_dir().join(format!(
        "delivery-outcomes-{}.sqlite",
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
    // Act: known temporary rejection allows a later attempt, never an immediate busy retry.
    if !store
        .complete_delivery(DeliveryCompletion::<_, _, String> {
            delivery_id: id.clone(),
            attempt_id: first.attempt_id.clone(),
            effects: effects(SubmissionEffect::Rejected),
            result: DeliveryResult::KnownNotSubmitted {
                reason: "temporary unavailable".into(),
                retryable: true,
            },
            now_ms: 1000,
        })
        .await?
    {
        return Err("known rejection was not recorded".into());
    }
    if store
        .claim_delivery::<String, String, String>(&id, 1000)
        .await?
        .is_some()
    {
        return Err("retry did not back off".into());
    }
    let next = store
        .claim_delivery::<String, String, String>(&id, 100000)
        .await?
        .ok_or("known non-submission could not retry")?;
    if next.attempt_id == first.attempt_id {
        return Err("retry reused attempt identity".into());
    }
    if !store
        .complete_delivery(DeliveryCompletion::<_, _, String> {
            delivery_id: id.clone(),
            attempt_id: next.attempt_id,
            effects: effects(SubmissionEffect::Unknown),
            result: DeliveryResult::Unknown {
                reason: "response lost".into(),
            },
            now_ms: 100000,
        })
        .await?
    {
        return Err("unknown outcome not recorded".into());
    }
    // Assert: even a much later worker cannot replay uncertainty or accept stale completion.
    if store
        .claim_delivery::<String, String, String>(&id, 999999)
        .await?
        .is_some()
    {
        return Err("uncertain input was replayed".into());
    }
    if store
        .complete_delivery(DeliveryCompletion {
            delivery_id: id,
            attempt_id: first.attempt_id,
            effects: effects(SubmissionEffect::Accepted),
            result: DeliveryResult::Accepted {
                receipt: "old receipt".to_owned(),
            },
            now_ms: 999999,
        })
        .await?
    {
        return Err("stale attempt changed current evidence".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn pause_during_dispatch_prevents_late_rejection_from_resurrecting_message()
-> Result<(), Box<dyn std::error::Error>> {
    use automation_storage::{WakeAction, WakeMutation};
    // Arrange: a fired message is already claimed when pause arrives.
    let path = std::env::temp_dir().join(format!(
        "delivery-pause-race-{}.sqlite",
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
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let id = match store
        .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 60000)
        .await?
    {
        WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
        _ => return Err("missing fire".into()),
    };
    let claim = store
        .claim_delivery::<String, String, String>(&id, 60000)
        .await?
        .ok_or("missing claim")?;
    store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id.clone(),
            action: WakeAction::Pause,
            now_ms: 61000,
        })
        .await?;
    store
        .mutate_wakeup::<Message>(&WakeMutation {
            operation_id: OperationId::generate(),
            wakeup_id: wake.definition.wakeup_id,
            action: WakeAction::Resume,
            now_ms: 62000,
        })
        .await?;
    // Act: the old dispatch finally reports that no input was submitted.
    store
        .complete_delivery(DeliveryCompletion::<_, _, String> {
            delivery_id: id.clone(),
            attempt_id: claim.attempt_id,
            effects: effects(SubmissionEffect::Rejected),
            result: DeliveryResult::KnownNotSubmitted {
                reason: "late temporary rejection".into(),
                retryable: true,
            },
            now_ms: 63000,
        })
        .await?;
    // Assert: resume enables future ticks, not retry of the message cancelled by pause.
    if store
        .claim_delivery::<String, String, String>(&id, 100000)
        .await?
        .is_some()
    {
        return Err("paused message was resurrected by a late rejection".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn pause_preserves_late_acceptance_and_unknown_effects()
-> Result<(), Box<dyn std::error::Error>> {
    use automation_storage::{WakeAction, WakeMutation};
    // Each case owns a fresh database; these are storage tests, not native model proof.
    for accepted in [true, false] {
        let path = std::env::temp_dir().join(format!(
            "delivery-pause-evidence-{}.sqlite",
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
                expiry: ExpiryRule::None,
                now_ms: 0,
            })
            .await?;
        let id = match store
            .evaluate_wakeup::<Message>(&wake.definition.wakeup_id, 60000)
            .await?
        {
            WakeEvaluation::Fired { delivery_id, .. } => delivery_id,
            _ => return Err("missing fire".into()),
        };
        let claim = store
            .claim_delivery::<String, String, String>(&id, 60000)
            .await?
            .ok_or("missing claim")?;
        store
            .mutate_wakeup::<Message>(&WakeMutation {
                operation_id: OperationId::generate(),
                wakeup_id: wake.definition.wakeup_id.clone(),
                action: WakeAction::Pause,
                now_ms: 61000,
            })
            .await?;
        // Resolve after reopening so pause intent cannot depend on memory or retained events.
        store.close().await?;
        let mut store = AutomationStore::open(&path).await?;
        let (effect, result) = if accepted {
            (
                SubmissionEffect::Accepted,
                DeliveryResult::Accepted {
                    receipt: "native receipt".to_owned(),
                },
            )
        } else {
            (
                SubmissionEffect::Unknown,
                DeliveryResult::Unknown {
                    reason: "response lost".into(),
                },
            )
        };
        if !store
            .complete_delivery(DeliveryCompletion {
                delivery_id: id.clone(),
                attempt_id: claim.attempt_id,
                effects: effects(effect),
                result,
                now_ms: 62000,
            })
            .await?
        {
            return Err("late evidence was not recorded".into());
        }
        if store
            .claim_delivery::<String, String, String>(&id, 999999)
            .await?
            .is_some()
        {
            return Err("pause caused accepted or uncertain input to replay".into());
        }
        let record = store
            .read_wakeup::<Message>(&wake.definition.wakeup_id)
            .await?;
        if accepted != record.pending_delivery_id.is_none() {
            return Err("pending identity does not reflect actual native evidence".into());
        }
        store.close().await?;
        std::fs::remove_file(path)?;
    }
    Ok(())
}
