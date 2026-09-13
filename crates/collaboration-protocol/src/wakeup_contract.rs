//! Timed wake creation is durable local acceptance, distinct from native message acceptance.
use crate::{
    CodexGeneration, ExpiryRequest, MessageContent, MessageDelivery, ObservationTimestamp,
    SessionRef, TimingRequest,
};
use agent_automation::{ChangeId, DeliveryId, OccurrenceId, OperationId, WakeupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavedMessage {
    pub target: SessionRef,
    pub content: MessageContent,
    pub delivery: MessageDelivery,
    #[serde(deserialize_with = "Option::deserialize")]
    pub generation_guard: Option<CodexGeneration>,
}
impl agent_automation::DurableMessage for SavedMessage {
    type Target = SessionRef;
    type Content = MessageContent;
    type Generation = CodexGeneration;
    fn target(&self) -> &Self::Target {
        &self.target
    }
    fn content(&self) -> &Self::Content {
        &self.content
    }
    fn generation_guard(&self) -> Option<&Self::Generation> {
        self.generation_guard.as_ref()
    }
    fn delivery_mode(&self) -> &'static str {
        match self.delivery {
            MessageDelivery::Auto => "auto",
            MessageDelivery::Steer => "steer",
            MessageDelivery::Queue => "queue",
        }
    }
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeSendRequest {
    pub operation_id: OperationId,
    pub message: SavedMessage,
    pub timing: TimingRequest,
    pub expiry: ExpiryRequest,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeShowRequest {
    pub wakeup_id: WakeupId,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeMutationRequest {
    pub operation_id: OperationId,
    pub wakeup_id: WakeupId,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeState {
    Active,
    Paused,
    Cancelled,
    Expired,
    Finished,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeDefinition {
    pub wakeup_id: WakeupId,
    pub change_id: ChangeId,
    pub message: SavedMessage,
    pub timing: TimingRequest,
    pub anchor_at: ObservationTimestamp,
    #[serde(deserialize_with = "Option::deserialize")]
    pub expires_at: Option<ObservationTimestamp>,
    pub created_at: ObservationTimestamp,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FireKind {
    WakeFired,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FireReceipt {
    pub kind: FireKind,
    pub wakeup_id: WakeupId,
    pub occurrence_id: OccurrenceId,
    pub due_at: ObservationTimestamp,
    pub fired_at: ObservationTimestamp,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeSnapshot {
    pub definition: WakeDefinition,
    pub state: WakeState,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_due_at: Option<ObservationTimestamp>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub first_fire: Option<FireReceipt>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub pending_delivery_id: Option<DeliveryId>,
    pub latest_event_cursor: String,
}
