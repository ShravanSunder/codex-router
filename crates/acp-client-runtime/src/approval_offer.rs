//! ACP-edge permission presentation and offered options before Host policy.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalArgument {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPresentation {
    pub tool_name: Option<String>,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub arguments: Vec<ApprovalArgument>,
    pub permission_details: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExternalApprovalOptionScope {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unsupported { provider_kind: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalApprovalOption {
    pub option_id: String,
    pub label: Option<String>,
    pub scope: ExternalApprovalOptionScope,
}
