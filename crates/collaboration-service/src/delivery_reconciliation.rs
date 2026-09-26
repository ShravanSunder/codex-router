//! Reconcile an uncertain wake only through the client named by its stored evidence.
use crate::{
    AttemptReconciliation, AttemptReconciliationContext, SessionMessageDelivery,
    stored_delivery_receipt::StoredDeliveryReceipt,
};
use agent_automation::{DeliveryStatus, RouteEffectEvidence, SubmissionEffect};
use automation_storage::{
    AutomationStore, DeliveryCompletion, DeliveryRecord, DeliveryResult, StorageError,
};
use collaboration_protocol::{
    CodexGeneration, DeliveryClientReceipt, DeliveryOutcome, MessageContent, MessageDelivery,
    NativeSendAcceptance, SessionReachability, SessionRef,
};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) async fn reconcile(
    store: &Arc<Mutex<AutomationStore>>,
    delivery: &dyn SessionMessageDelivery,
    record: DeliveryRecord<SessionRef, CodexGeneration, StoredDeliveryReceipt>,
) -> Result<(), StorageError> {
    if record.status != DeliveryStatus::Uncertain {
        return Ok(());
    }
    let Some(attempt) = record.attempt else {
        return Err(StorageError::InvalidRecord);
    };
    let Some(mut effects) = attempt.effects else {
        return Err(StorageError::InvalidRecord);
    };
    let mode = match record.mode.as_str() {
        "auto" => MessageDelivery::Auto,
        "queue" => MessageDelivery::Queue,
        "steer" => MessageDelivery::Steer,
        _ => return Err(StorageError::InvalidRecord),
    };
    let content = store
        .lock()
        .await
        .read_delivery_content::<MessageContent>(&record.delivery_id)
        .await?;
    let context = AttemptReconciliationContext {
        target: record.target,
        message: content,
        mode,
        recorded: effects.clone(),
    };
    let result = delivery
        .reconcile_attempt(context)
        .await
        .map_err(|_| StorageError::InvalidRecord)?;
    let completion = match result {
        AttemptReconciliation::Accepted(receipt) => {
            if !apply_accepted_evidence(&mut effects, &receipt) {
                return Ok(());
            }
            let effect =
                crate::delivery_acceptance_effect::accepted_delivery_effect(&receipt.outcome)
                    .ok_or(StorageError::InvalidRecord)?;
            DeliveryResult::Accepted {
                effect,
                receipt: *receipt,
            }
        }
        AttemptReconciliation::KnownNotSubmitted => {
            let reachability = match &effects {
                RouteEffectEvidence::CodexAppServer(_) => SessionReachability::CodexAppServer,
                RouteEffectEvidence::ProviderAcp(_) => SessionReachability::ProviderAcp,
                RouteEffectEvidence::ClaudeCodePeer(_) => SessionReachability::ClaudeCodePeer,
            };
            match &mut effects {
                RouteEffectEvidence::CodexAppServer(native) => {
                    native.submission = SubmissionEffect::NotDispatched;
                }
                RouteEffectEvidence::ProviderAcp(provider) => {
                    provider.submission = SubmissionEffect::NotDispatched;
                }
                RouteEffectEvidence::ClaudeCodePeer(_) => return Ok(()),
            }
            DeliveryResult::KnownNotSubmitted {
                reason: "Owning client proved the attempt was not submitted.".into(),
                retryable: true,
                receipt: Some(collaboration_protocol::DeliveryReceipt {
                    outcome: DeliveryOutcome::NotSubmitted {
                        retryable: true,
                        reason: "Owning client proved the attempt was not submitted.".into(),
                    },
                    reachability: Some(reachability),
                    client: None,
                }),
            }
        }
        AttemptReconciliation::StillUnknown => return Ok(()),
    };
    store
        .lock()
        .await
        .complete_delivery(DeliveryCompletion {
            delivery_id: record.delivery_id,
            attempt_id: attempt.attempt_id,
            effects: Some(effects),
            result: completion,
            now_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await?;
    Ok(())
}

fn apply_accepted_evidence(
    evidence: &mut RouteEffectEvidence<SessionRef, CodexGeneration>,
    receipt: &collaboration_protocol::DeliveryReceipt,
) -> bool {
    match (evidence, &receipt.outcome, &receipt.client) {
        (
            RouteEffectEvidence::CodexAppServer(native),
            DeliveryOutcome::Queued,
            Some(DeliveryClientReceipt::CodexAppServer(client)),
        ) => {
            let NativeSendAcceptance::QueueAccepted { submission_id } = &client.acceptance else {
                return false;
            };
            if native.target.as_ref() != Some(&client.target)
                || native.generation.as_ref() != Some(&client.generation)
                || native.client_user_message_id.as_deref()
                    != Some(String::from(client.client_user_message_id.clone()).as_str())
            {
                return false;
            }
            native.submission = SubmissionEffect::Accepted;
            native.native_submission_id = Some(String::from(submission_id.clone()));
            true
        }
        (
            RouteEffectEvidence::ProviderAcp(provider),
            DeliveryOutcome::Queued,
            Some(DeliveryClientReceipt::ProviderAcp { operation_id }),
        ) if provider.attempt_id.as_str() == operation_id.as_str()
            && provider.submission == SubmissionEffect::RouterQueued =>
        {
            true
        }
        (
            RouteEffectEvidence::ProviderAcp(provider),
            DeliveryOutcome::Started | DeliveryOutcome::Steered,
            Some(DeliveryClientReceipt::ProviderAcp { operation_id }),
        ) if provider.attempt_id.as_str() == operation_id.as_str()
            && provider.submission != SubmissionEffect::RouterQueued =>
        {
            provider.submission = SubmissionEffect::Accepted;
            true
        }
        _ => false,
    }
}
