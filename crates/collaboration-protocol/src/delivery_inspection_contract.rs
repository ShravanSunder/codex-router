//! Delivery eligibility and native evidence are separate, explicit inspection dimensions.
use crate::{
    CodexGeneration, MessageDelivery, NativeSendReceipt, ObservationTimestamp, SessionRef,
};
use agent_automation::{AttemptId, DeliveryId, OccurrenceId, WakeupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeEffectEvidence {
    #[serde(deserialize_with = "Option::deserialize")]
    pub target: Option<SessionRef>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub generation: Option<CodexGeneration>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub client_user_message_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub native_turn_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub native_submission_id: Option<String>,
    pub allocation: PreparationEffect,
    pub resume: PreparationEffect,
    pub submission: SubmissionEffect,
    pub cessation: CessationEvidence,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreparationEffect {
    NotRequested,
    NotDispatched,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmissionEffect {
    NotDispatched,
    Dispatching,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CessationEvidence {
    NotApplicable,
    Unconfirmed,
    Confirmed,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeliveryEvidence {
    NotDispatched,
    Dispatching {
        attempt_id: AttemptId,
        effects: NativeEffectEvidence,
    },
    KnownNotSubmitted {
        attempt_id: AttemptId,
        reason: String,
        effects: NativeEffectEvidence,
    },
    Accepted {
        attempt_id: AttemptId,
        receipt: NativeSendReceipt,
    },
    OutcomeUnknown {
        attempt_id: AttemptId,
        effects: NativeEffectEvidence,
        explanation: String,
    },
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DeliverySource {
    Wake {
        wakeup_id: WakeupId,
        occurrence_id: OccurrenceId,
    },
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryDisposition {
    Pending,
    Discarded,
    Failed,
    Accepted,
    Uncertain,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryInspection {
    pub delivery_id: DeliveryId,
    pub target: SessionRef,
    pub mode: MessageDelivery,
    pub source: DeliverySource,
    pub eligible_at: ObservationTimestamp,
    #[serde(deserialize_with = "Option::deserialize")]
    pub expires_at: Option<ObservationTimestamp>,
    pub disposition: DeliveryDisposition,
    pub evidence: DeliveryEvidence,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryShowRequest {
    pub delivery_id: DeliveryId,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeMutationResult {
    pub wakeup: crate::WakeSnapshot,
    pub discarded_delivery_ids: Vec<DeliveryId>,
    pub dispatched_deliveries: Vec<DeliveryInspection>,
}
