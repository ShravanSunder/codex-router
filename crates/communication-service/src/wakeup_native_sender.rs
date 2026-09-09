//! Timed messages reuse the ordinary generation-gated native message dispatcher directly.
use crate::{EndpointDirectory, NativeControlBackend};
use agent_automation::{
    CessationEvidence, DeliveryId, NativeEffectEvidence, PreparationEffect, SubmissionEffect,
};
use automation_storage::{
    AutomationStore, DeliveryClaim, DeliveryCompletion, DeliveryPreparation, DeliveryResult,
    StorageError,
};
use communication_protocol::{
    AcceptedResumeEffect, CodexGeneration, MessageContent, MessageDelivery, NativeSendAcceptance,
    NativeSendParams, NativeSendReceipt, SessionRef, UuidIdentity,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
#[cfg(test)]
#[path = "native_delivery_crash_tests.rs"]
mod crash_tests;

#[derive(Clone)]
pub(crate) struct WakeNativeSender {
    pub service_id: UuidIdentity,
    pub endpoints: EndpointDirectory,
    pub backend: Option<NativeControlBackend>,
    pub configuration: crate::AutomationConfigurationHandle,
}
impl WakeNativeSender {
    pub async fn dispatch(
        &self,
        store: Arc<Mutex<AutomationStore>>,
        id: DeliveryId,
    ) -> Result<(), StorageError> {
        let configuration_lease = self.configuration.admission_lease().await;
        if configuration_lease.configuration().is_none() {
            return Ok(());
        }
        let claim = store
            .lock()
            .await
            .claim_delivery::<SessionRef, MessageContent, CodexGeneration>(
                &id,
                chrono::Utc::now().timestamp_millis(),
            )
            .await?;
        drop(configuration_lease);
        let Some(claim) = claim else {
            return Ok(());
        };
        let mut effects = evidence(&claim);
        let selected = self
            .backend
            .as_ref()
            .and_then(|backend| backend.gate.acquire().ok())
            .map(|admission| admission.generation().clone());
        let result = if let Some(generation) = selected {
            let generation = claim.generation_guard.clone().unwrap_or(generation);
            effects.generation = Some(generation.clone());
            effects.resume = if claim.mode == "auto" {
                PreparationEffect::Unknown
            } else {
                PreparationEffect::NotRequested
            };
            effects.submission = SubmissionEffect::Dispatching;
            let prepared = store
                .lock()
                .await
                .prepare_delivery(DeliveryPreparation {
                    delivery_id: id.clone(),
                    attempt_id: claim.attempt_id.clone(),
                    effects: effects.clone(),
                })
                .await?;
            if !prepared {
                return Ok(());
            }
            #[cfg(test)]
            crash_tests::checkpoint("intent-persisted");
            let endpoints = self
                .endpoints
                .subscribe()
                .and_then(|subscription| subscription.snapshot())
                .map_err(|_| StorageError::InvalidRecord)?
                .endpoints;
            let mode = match claim.mode.as_str() {
                "auto" => MessageDelivery::Auto,
                "queue" => MessageDelivery::Queue,
                "steer" => MessageDelivery::Steer,
                _ => return Err(StorageError::InvalidRecord),
            };
            let params = NativeSendParams {
                target: claim.target.clone(),
                generation,
                message: claim.content,
                delivery: mode,
                client_user_message_id: Some(
                    id.as_str()
                        .to_owned()
                        .try_into()
                        .map_err(|_| StorageError::InvalidRecord)?,
                ),
            };
            let response = crate::native_message_dispatch::dispatch_message(
                crate::native_control_dispatch::NativeControlRequest {
                    method: "codex/messageSend",
                    params: serde_json::to_value(params)
                        .map_err(|_| StorageError::InvalidRecord)?,
                    id: json!(claim.attempt_id),
                    service_id: &self.service_id,
                    backend: self.backend.as_ref(),
                    endpoints: &endpoints,
                    stored_observation: None,
                },
            )
            .await;
            interpret_response(response, &mut effects, claim.generation_guard.is_some())
        } else {
            DeliveryResult::KnownNotSubmitted{reason:"Native backend unavailable; no native call was dispatched. The same delivery will retry when eligible.".into(),retryable:true}
        };
        #[cfg(test)]
        if matches!(&result, DeliveryResult::Accepted { .. }) {
            crash_tests::checkpoint("receipt-before-commit");
        }
        store
            .lock()
            .await
            .complete_delivery(DeliveryCompletion {
                delivery_id: id,
                attempt_id: claim.attempt_id,
                effects,
                result,
                now_ms: chrono::Utc::now().timestamp_millis(),
            })
            .await?;
        Ok(())
    }
}
fn evidence(
    claim: &DeliveryClaim<SessionRef, MessageContent, CodexGeneration>,
) -> NativeEffectEvidence<SessionRef, CodexGeneration> {
    NativeEffectEvidence {
        target: Some(claim.target.clone()),
        generation: None,
        client_user_message_id: Some(claim.delivery_id.as_str().into()),
        native_turn_id: None,
        native_submission_id: None,
        allocation: PreparationEffect::NotRequested,
        resume: PreparationEffect::NotRequested,
        submission: SubmissionEffect::NotDispatched,
        cessation: CessationEvidence::NotApplicable,
    }
}
fn interpret_response(
    response: Value,
    effects: &mut NativeEffectEvidence<SessionRef, CodexGeneration>,
    strict_generation: bool,
) -> DeliveryResult<NativeSendReceipt> {
    if let Some(result) = response.get("result") {
        let receipt = serde_json::from_value::<NativeSendReceipt>(result.clone());
        if let Ok(receipt) = receipt {
            if effects.target.as_ref() != Some(&receipt.target)
                || effects.generation.as_ref() != Some(&receipt.generation)
                || effects.client_user_message_id.as_deref()
                    != Some(String::from(receipt.client_user_message_id.clone()).as_str())
            {
                return unknown(
                    effects,
                    "Native receipt identity disagrees with this delivery; inspect the exact attempt.",
                );
            }
            effects.resume = match receipt.resume_effect {
                AcceptedResumeEffect::Accepted => PreparationEffect::Accepted,
                AcceptedResumeEffect::NotRequested => PreparationEffect::NotRequested,
            };
            effects.submission = SubmissionEffect::Accepted;
            match &receipt.acceptance {
                NativeSendAcceptance::QueueAccepted { submission_id } => {
                    effects.native_submission_id = Some(String::from(submission_id.clone()))
                }
                NativeSendAcceptance::NativeInputAccepted { turn_id, .. } => {
                    effects.native_turn_id = Some(String::from(turn_id.clone()))
                }
                NativeSendAcceptance::SteerAccepted {
                    turn_id,
                    submission_id,
                } => {
                    effects.native_turn_id = Some(String::from(turn_id.clone()));
                    effects.native_submission_id = submission_id.clone().map(String::from);
                }
            }
            return DeliveryResult::Accepted { receipt };
        }
        return unknown(
            effects,
            "Native result was malformed; acceptance is unknown and will not be replayed.",
        );
    }
    if !communication_protocol::control_error_is_valid("codex/messageSend", &response) {
        return unknown(
            effects,
            "Native error evidence was malformed; reconcile before resending.",
        );
    }
    let Some(data) = response.pointer("/error/data") else {
        return unknown(
            effects,
            "Native failure omitted effect evidence; inspect before resending.",
        );
    };
    effects.resume = match data.pointer("/effects/resume").and_then(Value::as_str) {
        Some("notRequested") => PreparationEffect::NotRequested,
        Some("accepted") => PreparationEffect::Accepted,
        Some("rejected") => PreparationEffect::Rejected,
        _ => PreparationEffect::Unknown,
    };
    effects.submission = match data.pointer("/effects/submission").and_then(Value::as_str) {
        Some("notDispatched") => SubmissionEffect::NotDispatched,
        Some("rejected") => SubmissionEffect::Rejected,
        _ => SubmissionEffect::Unknown,
    };
    if effects.resume == PreparationEffect::Unknown
        || effects.submission == SubmissionEffect::Unknown
    {
        return DeliveryResult::Unknown {
            reason: "Native effects are uncertain; inspect this delivery before any resend.".into(),
        };
    }
    let kind = data
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("invalidRecord");
    let retryable = matches!(kind, "unavailable" | "overloaded")
        || (kind == "staleGeneration" && !strict_generation);
    let reason = match kind {
        "threadNotLoaded" => {
            "Explicit queue requires a loaded thread; resume the target before creating another request."
        }
        "noActiveTurn" => {
            "Explicit steer requires an active turn; inspect the target and choose the intended delivery mode."
        }
        "staleGeneration" => {
            "Native generation changed; inspect endpoint generation and the saved strict guard."
        }
        "unsupportedCapability" => {
            "Endpoint cannot perform this operation; inspect its advertised capabilities."
        }
        "nativeRejected" => {
            "Native backend rejected the operation; inspect the exact target and selected delivery mode."
        }
        _ => {
            "Native input was not submitted; inspect target availability and this delivery's retry eligibility."
        }
    };
    DeliveryResult::KnownNotSubmitted {
        reason: reason.into(),
        retryable,
    }
}
fn unknown(
    effects: &mut NativeEffectEvidence<SessionRef, CodexGeneration>,
    reason: &str,
) -> DeliveryResult<NativeSendReceipt> {
    effects.submission = SubmissionEffect::Unknown;
    DeliveryResult::Unknown {
        reason: reason.into(),
    }
}
