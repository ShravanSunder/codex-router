//! Closed reasons and actions keep a rejected delivery actionable across clients.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryRejectionReason {
    ChildThread,
    Busy,
    HeldByAnotherClient,
    SettingsUnresolved,
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
    ProviderSessionNotFound,
    ProviderRejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryNextAction {
    InspectTarget,
    UseDeliverySteer,
    MessageFromHoldingCodexClient,
    RequestApproval,
    CorrectRequest,
    RetryLater,
}

/// One live Claude registry record claiming a target session.
#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryPeerClaim {
    pub pid: u32,
    pub name: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryRejection {
    pub reason: DeliveryRejectionReason,
    pub next_action: DeliveryNextAction,
    pub client_code: Option<i64>,
    pub detail: Option<String>,
    /// Full claims for an ambiguous live Claude session; absent for other rejections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claims: Option<Vec<DeliveryPeerClaim>>,
}
