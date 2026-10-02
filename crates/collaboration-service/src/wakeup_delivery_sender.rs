//! Timed wake attempts use the injected delivery seam and persist effect intent first.
use crate::{
    AttemptEvidenceSink, DeliveryContractError, DeliveryFuture, DeliveryPrecondition, LoadPolicy,
    SessionMessageDelivery,
};
use agent_automation::{DeliveryId, FirstFire, RouteEffectEvidence, WakeRecord};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryPreparation, DeliveryResult, PushRecordDraft,
    StorageError,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryCorrelationId, DeliveryOutcome, MachineId, MessageContent,
    MessageDelivery, MessageText, PushDeliveryState, PushHeaderFacts, PushId, PushKind,
    PushLineInput, PushOrigin, PushRecord, RouterLink, RouterOriginRef, SavedMessage, SessionRef,
    render_push_line,
};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(test)]
#[path = "wakeup_delivery_sender/aged_wake_retention_tests.rs"]
mod aged_wake_retention_tests;
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
    pub machine_identity: crate::MachineIdentity,
}

pub(crate) fn build_wake_push_draft(
    wake: &WakeRecord<SavedMessage>,
    fire: &FirstFire,
) -> Result<PushRecordDraft, StorageError> {
    if fire.wakeup_id != wake.definition.wakeup_id {
        return Err(StorageError::InvalidRecord);
    }
    let body = match &wake.definition.message.content {
        MessageContent::Agent { text, .. }
        | MessageContent::HumanUser { text }
        | MessageContent::Router { text } => text.as_str().to_owned(),
    };
    let origin_router_ref = RouterOriginRef::Wake {
        wakeup_id: fire.wakeup_id.clone(),
        occurrence_id: fire.occurrence_id.clone(),
    }
    .canonical_string()
    .map_err(|_| StorageError::InvalidRecord)?;
    let push_id = PushId::try_from(uuid::Uuid::now_v7().to_string())
        .map_err(|_| StorageError::InvalidRecord)?;
    let created_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(fire.fired_at_ms)
        .ok_or(StorageError::InvalidRecord)?;
    Ok(PushRecordDraft {
        mode: None,
        guard: None,
        push_id,
        kind: PushKind::Wake,
        origin: PushOrigin::Router(PushKind::Wake),
        origin_router_ref: Some(origin_router_ref),
        target: wake.definition.message.target.clone(),
        reply_to_push_id: None,
        header_facts: PushHeaderFacts::Wake,
        body: Some(body),
        activity: None,
        created_at,
    })
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
        let delivery_record = store
            .lock()
            .await
            .read_delivery::<SessionRef, CodexGeneration, serde_json::Value>(&delivery_id)
            .await?;
        if delivery_record.wakeup_id != claim.wakeup_id {
            return Err(StorageError::InvalidRecord);
        }
        let origin = RouterOriginRef::Wake {
            wakeup_id: claim.wakeup_id.clone(),
            occurrence_id: delivery_record.occurrence_id,
        };
        let push = store
            .lock()
            .await
            .get_push_record_by_origin_reference(&origin)
            .await?
            .ok_or(StorageError::PushNotFound)?;
        if push.kind != PushKind::Wake
            || push.origin != PushOrigin::Router(PushKind::Wake)
            || push.target != claim.target
            || push.body.as_deref() != Some(message_body(&claim.content))
        {
            return Err(StorageError::InvalidRecord);
        }
        let prepared_push = prepared_wake_push(&push, &self.machine_identity)?;
        let push_id = push.push_id.clone();
        {
            let mut storage = store.lock().await;
            if push.delivery_state == PushDeliveryState::Attempted {
                storage
                    .restore_push_pending_after_not_started(&push_id)
                    .await?;
            } else if push.delivery_state != PushDeliveryState::Pending {
                return Err(StorageError::InvalidRecord);
            }
            storage.mark_push_attempted(&push_id).await?;
        }
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
        let request = crate::layer_zero::DeliveryRequest {
            payload: prepared_push,
            target: claim.target,
            mode,
            precondition: claim
                .generation_guard
                .map_or(DeliveryPrecondition::Unpinned, |expected| {
                    DeliveryPrecondition::EndpointGeneration { expected }
                }),
            correlation: DeliveryCorrelationId::try_from(push_id.as_str().to_owned())
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
        if matches!(
            receipt.outcome,
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                ..
            }
        ) {
            store
                .lock()
                .await
                .restore_push_pending_after_not_started(&push_id)
                .await?;
        } else {
            store
                .lock()
                .await
                .settle_push_record(&push_id, receipt.clone(), chrono::Utc::now())
                .await?;
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

fn message_body(content: &MessageContent) -> &str {
    match content {
        MessageContent::Agent { text, .. }
        | MessageContent::HumanUser { text }
        | MessageContent::Router { text } => text.as_str(),
    }
}

fn prepared_wake_push(
    record: &PushRecord,
    machine_identity: &crate::MachineIdentity,
) -> Result<crate::layer_zero::PreparedPush, StorageError> {
    let rendered_line = render_push_line(&PushLineInput {
        link: RouterLink::new(
            MachineId::from(machine_identity.service_id().clone()),
            record.push_id.clone(),
        ),
        machine_label: machine_identity.machine_label().clone(),
        origin: record.origin.clone(),
        header_facts: record.header_facts.clone(),
        body: record.body.clone(),
    })
    .map_err(|_| StorageError::InvalidRecord)?;
    let line: MessageText = rendered_line
        .try_into()
        .map_err(|_| StorageError::InvalidRecord)?;
    Ok(crate::layer_zero::PreparedPush {
        push_id: record.push_id.clone(),
        line,
        load_policy: LoadPolicy::MayLoad,
    })
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
