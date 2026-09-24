//! Closed reasons and actions keep a rejected delivery actionable across clients.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryRejectionReason {
    ChildThread,
    Busy,
    NotResumable,
    PermissionDenied,
    UnsupportedCapability,
    Unknown,
    EndpointUnavailable,
    NoRoute,
    LiveElsewhere,
    SteerUnsupported,
    QueueUnsupported,
    StaleGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryNextAction {
    InspectTarget,
    UseDeliverySteer,
    RequestApproval,
    CorrectRequest,
    RetryLater,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryRejection {
    pub reason: DeliveryRejectionReason,
    pub next_action: DeliveryNextAction,
    pub client_code: Option<i64>,
    pub detail: Option<String>,
}
