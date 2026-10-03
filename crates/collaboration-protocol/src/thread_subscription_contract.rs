//! Control subscription requests and service-assembled observation views.
use chrono::{DateTime, Utc};
use message_board::{
    EndReason, Identity, SubscriptionDeliveryOutcome, SubscriptionPolicy, SubscriptionPolicyPatch,
    SubscriptionScope,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscribeRequest {
    pub actor: Identity,
    pub scope: SubscriptionScope,
    pub policy: SubscriptionPolicyPatch,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadUnsubscribeRequest {
    pub actor: Identity,
    pub scope: SubscriptionScope,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionsRequest {
    pub actor: Identity,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionsResult {
    pub subscriptions: Vec<ThreadSubscriptionView>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadSubscriptionState {
    Active,
    Draining,
    Ended,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadSubscriptionPresence {
    Running {},
    Wakeable {},
    Unreachable { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionView {
    pub scope: SubscriptionScope,
    pub policy: SubscriptionPolicy,
    pub state: ThreadSubscriptionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_reason: Option<EndReason>,
    pub expires_at: DateTime<Utc>,
    pub pending_count: u64,
    pub presence: ThreadSubscriptionPresence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_since: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<SubscriptionDeliveryOutcome>,
}
