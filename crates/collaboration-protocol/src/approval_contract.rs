//! Client-exposed Codex approval routing and single-use decisions.
use crate::{CodexGeneration, SessionRef};
use message_board::Identity;
use serde::{Deserialize, Serialize};

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(
    schemars::JsonSchema, Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Allow,
    AllowForSession,
    Deny,
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalState {
    PendingClientDecision,
    TimedOut,
    ApproverUnreachable,
    ApproverIsRequester,
    Cancelled,
    Decided,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ApprovalOptionScope {
    AllowOnce,
    AllowForSession,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unsupported { provider_kind: String },
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalOfferedOption {
    pub option_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub scope: ApprovalOptionScope,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalArgument {
    pub name: String,
    pub value: String,
}

#[derive(schemars::JsonSchema, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPresentation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ApprovalArgument>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permission_details: Vec<String>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalListParams {
    pub pending: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub include_options: bool,
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalOptionEffect {
    Allow,
    Decline,
    Abort,
}

#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalOptionViewScope {
    Once,
    Session,
    Persistent,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalOptionView {
    pub option_id: String,
    pub label: String,
    pub effect: ApprovalOptionEffect,
    pub scope: ApprovalOptionViewScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persistent_target: Option<String>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDetailedRecord {
    pub request_id: String,
    pub requester: message_board::SessionRef,
    pub approver: Identity,
    pub state: ApprovalState,
    pub title: Option<String>,
    pub description: Option<String>,
    pub options: Vec<ApprovalOptionView>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDetailedListResult {
    pub approvals: Vec<ApprovalDetailedRecord>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ApprovalListResponse {
    Legacy(ApprovalListResult),
    Detailed(ApprovalDetailedListResult),
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequestRecord {
    pub request_id: String,
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub generation: CodexGeneration,
    pub state: ApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offered_options: Vec<ApprovalOfferedOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presentation: Option<ApprovalPresentation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    pub operation: serde_json::Value,
    pub expires_at: String,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalListResult {
    pub approvals: Vec<ApprovalRequestRecord>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideParams {
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub acknowledge_persistent: bool,
    #[serde(deserialize_with = "crate::interaction_actor::deserialize_interaction_actor")]
    pub actor: Identity,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideResult {
    pub request_id: String,
    pub state: ApprovalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<ApprovalDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option_id: Option<String>,
    pub scope: Option<String>,
}
