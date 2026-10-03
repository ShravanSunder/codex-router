//! Permission prompt and subject metadata on the versioned profile wire.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalPromptMetadata {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// ACP permission-request `_meta` shared by the client and agent faces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequestProfileMetadata {
    pub session_profile: ApprovalRequestProfileFields,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequestProfileFields {
    pub prompt: ApprovalPromptMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<crate::ApprovalSubject>,
}

impl ApprovalRequestProfileMetadata {
    #[must_use]
    pub fn from_request(request: &crate::ApprovalRequest) -> Self {
        Self {
            session_profile: ApprovalRequestProfileFields {
                prompt: ApprovalPromptMetadata {
                    title: request.title.clone(),
                    description: request.description.clone(),
                },
                subject: request.subject.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalSubject {
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ToolCallSubject,
    },
    Command {
        command: String,
        cwd: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        terminal_id: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallSubject {
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// ACP tool-call updates can add fields beyond this profile's identity.
    #[serde(flatten)]
    pub additional_fields: BTreeMap<String, Value>,
}
