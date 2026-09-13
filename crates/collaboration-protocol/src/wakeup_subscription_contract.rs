//! A dedicated wake subscription observes firing; it never waits for native acceptance.
use crate::{FireReceipt, WakeSnapshot};
use agent_automation::{SubscriptionId, WakeupId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeSubscription {
    pub subscription_id: SubscriptionId,
    pub snapshot: WakeSnapshot,
    pub after: String,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WakeChange {
    Fired { fire: FireReceipt },
    Paused,
    Cancelled,
    Expired,
    FinishedWithoutFiring,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeChanged {
    pub subscription_id: SubscriptionId,
    pub cursor: String,
    pub wakeup_id: WakeupId,
    pub change: WakeChange,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitUnavailable {
    pub kind: WaitUnavailableKind,
    pub stage: WaitStage,
    pub message: String,
    pub wakeup_id: WakeupId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub first_occurrence_id: Option<agent_automation::OccurrenceId>,
    pub effects: WaitUnavailableEffects,
    pub next_action: WaitNextAction,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitUnavailableKind {
    WaitUnavailable,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitStage {
    WaitForFirstFire,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WaitNextAction {
    ReconnectWait,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitUnavailableEffects {
    pub first_fire: UnknownFire,
    pub wakeup_mutation: NoMutation,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownFire {
    Unknown,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NoMutation {
    None,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeNotFound {
    pub kind: WakeNotFoundKind,
    pub stage: WaitStage,
    pub message: String,
    pub wakeup_id: WakeupId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub first_occurrence_id: Option<agent_automation::OccurrenceId>,
    pub effects: UnknownFirstFire,
    pub next_action: VerifyWakeupAddress,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeNotFoundKind {
    WakeNotFound,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyWakeupAddress {
    VerifyWakeupAddress,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UnknownFirstFire {
    pub first_fire: UnknownFire,
}
