//! Timed wake attempts use the injected delivery seam and persist effect intent first.
use crate::{
    AttemptEvidenceSink, DeliveryContractError, DeliveryFuture, DeliveryPrecondition,
    DeliveryRequest, SessionMessageDelivery,
};
use agent_automation::{DeliveryId, RouteEffectEvidence};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryPreparation, DeliveryResult, StorageError,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, MessageContent, MessageDelivery,
    SessionRef,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "native_delivery_crash_tests.rs"]
mod crash_tests;
#[cfg(test)]
#[path = "wakeup_delivery_sender_tests.rs"]
mod tests;

#[derive(Clone)]
pub(crate) struct WakeDeliverySender {
    pub delivery: Arc<dyn SessionMessageDelivery>,
    pub configuration: crate::AutomationConfigurationHandle,
}

impl WakeDeliverySender {
    pub async fn dispatch(
        &self,
        store: Arc<Mutex<AutomationStore>>,
        delivery_id: DeliveryId,
    ) -> Result<(), StorageError> {
        let configuration_lease = self.configuration.admission_lease().await;
        if configuration_lease.configuration().is_none() {
            return Ok(());
        }
        let claim = store
            .lock()
            .await
            .claim_delivery::<SessionRef, MessageContent, CodexGeneration>(
                &delivery_id,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        drop(configuration_lease);
        let Some(claim) = claim else {
            return Ok(());
        };
        let mode = match claim.mode.as_str() {
            "auto" => MessageDelivery::Auto,
            "queue" => MessageDelivery::Queue,
            "steer" => MessageDelivery::Steer,
            _ => return Err(StorageError::InvalidRecord),
        };
        let sink = WakeEvidenceSink {
            store: Arc::clone(&store),
            delivery_id: delivery_id.clone(),
            attempt_id: claim.attempt_id.clone(),
            latest: Mutex::new(None),
        };
        let request = DeliveryRequest {
            target: claim.target,
            message: claim.content,
            mode,
            precondition: claim
                .generation_guard
                .map_or(DeliveryPrecondition::Unpinned, |expected| {
                    DeliveryPrecondition::EndpointGeneration { expected }
                }),
            correlation: DeliveryCorrelationId::try_from(delivery_id.as_str().to_owned())
                .map_err(|_| StorageError::InvalidRecord)?,
            attempt: claim.attempt_id.clone(),
        };
        let receipt = self
            .delivery
            .deliver(request, &sink)
            .await
            .map_err(|_| StorageError::InvalidRecord)?;
        #[cfg(test)]
        if matches!(
            receipt.outcome,
            DeliveryOutcome::Started
                | DeliveryOutcome::Steered
                | DeliveryOutcome::StartedOrSteered
                | DeliveryOutcome::Queued
                | DeliveryOutcome::PeerMessageWritten
        ) {
            crash_tests::checkpoint("receipt-before-commit");
        }
        let accepted_effect =
            crate::delivery_acceptance_effect::accepted_delivery_effect(&receipt.outcome);
        let result = match &receipt.outcome {
            DeliveryOutcome::Started
            | DeliveryOutcome::Steered
            | DeliveryOutcome::StartedOrSteered
            | DeliveryOutcome::Queued
            | DeliveryOutcome::PeerMessageWritten => DeliveryResult::Accepted {
                effect: accepted_effect.ok_or(StorageError::InvalidRecord)?,
                receipt,
            },
            DeliveryOutcome::NotSubmitted { retryable, reason } => {
                DeliveryResult::KnownNotSubmitted {
                    reason: reason.clone(),
                    retryable: *retryable,
                    receipt: Some(receipt),
                }
            }
            DeliveryOutcome::Rejected(rejection) => DeliveryResult::KnownNotSubmitted {
                reason: rejection
                    .detail
                    .clone()
                    .unwrap_or_else(|| format!("{:?}", rejection.reason)),
                retryable: false,
                receipt: Some(receipt),
            },
            DeliveryOutcome::Unknown => DeliveryResult::Unknown {
                reason: "Selected client outcome is unknown; reconcile this attempt.".into(),
                receipt: Some(receipt),
            },
        };
        store
            .lock()
            .await
            .complete_delivery(DeliveryCompletion {
                delivery_id,
                attempt_id: claim.attempt_id,
                effects: sink.latest.lock().await.clone(),
                result,
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await?;
        Ok(())
    }
}

struct WakeEvidenceSink {
    store: Arc<Mutex<AutomationStore>>,
    delivery_id: DeliveryId,
    attempt_id: agent_automation::AttemptId,
    latest: Mutex<Option<RouteEffectEvidence<SessionRef, CodexGeneration>>>,
}

impl AttemptEvidenceSink for WakeEvidenceSink {
    fn record(
        &self,
        evidence: RouteEffectEvidence<SessionRef, CodexGeneration>,
    ) -> DeliveryFuture<'_, ()> {
        Box::pin(async move {
            let mut latest = self.latest.lock().await;
            if latest.is_none() {
                let prepared = self
                    .store
                    .lock()
                    .await
                    .prepare_delivery(DeliveryPreparation {
                        delivery_id: self.delivery_id.clone(),
                        attempt_id: self.attempt_id.clone(),
                        effects: evidence.clone(),
                    })
                    .await
                    .map_err(|_| DeliveryContractError::EvidencePersistence)?;
                if !prepared {
                    return Err(DeliveryContractError::EvidencePersistence);
                }
                #[cfg(test)]
                crash_tests::checkpoint("intent-persisted");
            }
            *latest = Some(evidence);
            Ok(())
        })
    }
}
