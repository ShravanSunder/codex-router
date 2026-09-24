use agent_automation::{
    ClaudeCodePeerEffectEvidence, DeliveryStatus, DurableMessage, ExpiryRule, OperationId,
    PeerWriteEffect, ProviderAcpEffectEvidence, ProviderBindingReference, ProviderSettlementEffect,
    RouteEffectEvidence, SubmissionEffect, TimingRule,
};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryPreparation, DeliveryResult, WakeCreate,
    WakeEvaluation,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct StoredMessage {
    target: String,
    text: String,
}

impl DurableMessage for StoredMessage {
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

async fn claimed_attempt(
    store: &mut AutomationStore,
) -> Result<(agent_automation::DeliveryId, agent_automation::AttemptId), Box<dyn std::error::Error>>
{
    let wake = store
        .create_wakeup(&WakeCreate {
            operation_id: OperationId::generate(),
            message: StoredMessage {
                target: "target".into(),
                text: "hello".into(),
            },
            timing: TimingRule::After { seconds: 1 },
            expiry: ExpiryRule::None,
            now_ms: 0,
        })
        .await?;
    let WakeEvaluation::Fired { delivery_id, .. } = store
        .evaluate_wakeup::<StoredMessage>(&wake.definition.wakeup_id, 1000)
        .await?
    else {
        return Err("missing fire".into());
    };
    let claim = store
        .claim_delivery::<String, String, String>(&delivery_id, 1000)
        .await?
        .ok_or("missing claim")?;
    Ok((delivery_id, claim.attempt_id))
}

fn provider_evidence(
    attempt_id: agent_automation::AttemptId,
    submission: SubmissionEffect,
) -> Result<RouteEffectEvidence<String, String>, Box<dyn std::error::Error>> {
    Ok(RouteEffectEvidence::ProviderAcp(
        ProviderAcpEffectEvidence {
            target: "target".into(),
            generation: "generation-1".into(),
            binding: ProviderBindingReference::try_from("binding-1".to_owned())?,
            attempt_id,
            submission,
            settlement: ProviderSettlementEffect::NotObserved,
        },
    ))
}

fn peer_evidence(
    write: PeerWriteEffect,
) -> Result<RouteEffectEvidence<String, String>, Box<dyn std::error::Error>> {
    Ok(RouteEffectEvidence::ClaudeCodePeer(
        ClaudeCodePeerEffectEvidence {
            session_id: "target".to_owned().try_into()?,
            process_id: 42.try_into()?,
            write,
        },
    ))
}

#[tokio::test]
async fn provider_attempt_requires_matching_dispatch_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "provider-attempt-{}.sqlite",
        OperationId::generate().as_str()
    ));
    let mut store = AutomationStore::open(&path).await?;
    let (delivery_id, attempt_id) = claimed_attempt(&mut store).await?;
    let wrong_id = agent_automation::AttemptId::generate();
    if store
        .prepare_delivery(DeliveryPreparation {
            delivery_id: delivery_id.clone(),
            attempt_id: attempt_id.clone(),
            effects: provider_evidence(attempt_id.clone(), SubmissionEffect::Accepted)?,
        })
        .await
        .is_ok()
    {
        return Err("provider operation was accepted before dispatch intent".into());
    }
    if store
        .prepare_delivery(DeliveryPreparation {
            delivery_id: delivery_id.clone(),
            attempt_id: attempt_id.clone(),
            effects: provider_evidence(wrong_id, SubmissionEffect::Dispatching)?,
        })
        .await
        .is_ok()
    {
        return Err("provider evidence admitted a different operation identity".into());
    }
    if !store
        .prepare_delivery(DeliveryPreparation {
            delivery_id: delivery_id.clone(),
            attempt_id: attempt_id.clone(),
            effects: provider_evidence(attempt_id.clone(), SubmissionEffect::Dispatching)?,
        })
        .await?
    {
        return Err("provider dispatch evidence was not recorded".into());
    }
    if store
        .complete_delivery(DeliveryCompletion::<_, _, String> {
            delivery_id: delivery_id.clone(),
            attempt_id: attempt_id.clone(),
            effects: peer_evidence(PeerWriteEffect::Written)?,
            result: DeliveryResult::Accepted {
                receipt: "wrong route".into(),
            },
            now_ms: 2000,
        })
        .await
        .is_ok()
    {
        return Err("provider attempt switched clients at completion".into());
    }
    if !store
        .complete_delivery(DeliveryCompletion {
            delivery_id: delivery_id.clone(),
            attempt_id: attempt_id.clone(),
            effects: provider_evidence(attempt_id, SubmissionEffect::Accepted)?,
            result: DeliveryResult::Accepted {
                receipt: "provider operation".to_owned(),
            },
            now_ms: 2000,
        })
        .await?
    {
        return Err("provider acceptance was not recorded".into());
    }
    let recorded = store
        .read_delivery::<String, String, String>(&delivery_id)
        .await?;
    if recorded.status != DeliveryStatus::Accepted
        || !matches!(
            recorded.attempt.and_then(|attempt| attempt.effects),
            Some(RouteEffectEvidence::ProviderAcp(_))
        )
    {
        return Err("provider acceptance lost its route evidence".into());
    }
    store.close().await?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
async fn peer_write_is_final_and_uncertain_write_never_retries()
-> Result<(), Box<dyn std::error::Error>> {
    for (write, result, expected) in [
        (
            PeerWriteEffect::Written,
            DeliveryResult::Accepted {
                receipt: "written".to_owned(),
            },
            DeliveryStatus::Accepted,
        ),
        (
            PeerWriteEffect::Unknown,
            DeliveryResult::Unknown {
                reason: "partial write".into(),
            },
            DeliveryStatus::Uncertain,
        ),
    ] {
        let path = std::env::temp_dir().join(format!(
            "peer-attempt-{}.sqlite",
            OperationId::generate().as_str()
        ));
        let mut store = AutomationStore::open(&path).await?;
        let (delivery_id, attempt_id) = claimed_attempt(&mut store).await?;
        if store
            .prepare_delivery(DeliveryPreparation {
                delivery_id: delivery_id.clone(),
                attempt_id: attempt_id.clone(),
                effects: peer_evidence(PeerWriteEffect::Written)?,
            })
            .await
            .is_ok()
        {
            return Err("peer write was accepted before dispatch intent".into());
        }
        if !store
            .prepare_delivery(DeliveryPreparation {
                delivery_id: delivery_id.clone(),
                attempt_id: attempt_id.clone(),
                effects: peer_evidence(PeerWriteEffect::Dispatching)?,
            })
            .await?
        {
            return Err("peer dispatch evidence was not recorded".into());
        }
        if !store
            .complete_delivery(DeliveryCompletion {
                delivery_id: delivery_id.clone(),
                attempt_id,
                effects: peer_evidence(write)?,
                result,
                now_ms: 2000,
            })
            .await?
        {
            return Err("peer write result was not recorded".into());
        }
        if store
            .read_delivery::<String, String, String>(&delivery_id)
            .await?
            .status
            != expected
        {
            return Err("peer write result changed delivery status incorrectly".into());
        }
        if store
            .claim_delivery::<String, String, String>(&delivery_id, 100_000)
            .await?
            .is_some()
        {
            return Err("a peer write was replayed".into());
        }
        store.close().await?;
        std::fs::remove_file(path)?;
    }
    Ok(())
}
