//! Client-exposed Codex approval routing and single-use decisions.
use crate::{CodexGeneration, SessionRef};
use serde::{Deserialize, Serialize};

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
    Cancelled,
    Decided,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalListParams {
    pub pending: bool,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequestRecord {
    pub request_id: String,
    pub requester: SessionRef,
    pub approver: SessionRef,
    pub generation: CodexGeneration,
    pub state: ApprovalState,
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
    pub decision: ApprovalDecision,
    pub actor: SessionRef,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalDecideResult {
    pub request_id: String,
    pub state: ApprovalState,
    pub decision: ApprovalDecision,
    pub scope: Option<String>,
}
