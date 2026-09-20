//! Public request, result, settlement, and failure types for ACP conversations.

use crate::ClientError;
use collaboration_protocol::{EndpointRef, MessageContent, SessionId, SessionRef, UuidIdentity};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub enum ConversationEvent {
    SessionReady(SessionRef),
    SessionUpdate {
        target: SessionRef,
        update: Value,
    },
    PermissionRequired(SessionRef),
    PromptResult {
        target: SessionRef,
        end: ConversationEnd,
        result: Value,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateRequest {
    pub endpoint: EndpointRef,
    pub cwd: std::path::PathBuf,
    pub session: Option<SessionId>,
    pub fork: Option<SessionId>,
    pub model: Option<String>,
    /// Absent on resume, where the thread keeps the effort it was created with,
    /// and on fork, where the source thread's effort is inherited.
    pub effort: Option<String>,
    pub access: Option<String>,
    pub created_by: Option<SessionRef>,
    pub approver: Option<SessionRef>,
    pub root_message_id: Option<UuidIdentity>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateResult {
    pub target: SessionRef,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPromptRequest {
    pub message: PublicPromptContent,
    pub effort: Option<String>,
    #[schemars(range(min = 1))]
    pub timeout_seconds: u64,
}

impl ConversationPromptRequest {
    pub fn validate(&self) -> Result<(), ClientError> {
        if self.timeout_seconds == 0 {
            return Err(ClientError::InvalidRequest(
                "prompt timeout must be at least one second",
            ));
        }
        tokio::time::Instant::now()
            .checked_add(Duration::from_secs(self.timeout_seconds))
            .ok_or(ClientError::InvalidRequest("invalid prompt deadline"))?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum PublicPromptContent {
    Agent {
        sender: SessionRef,
        text: collaboration_protocol::MessageText,
    },
    HumanUser {
        text: collaboration_protocol::MessageText,
    },
}

impl From<PublicPromptContent> for MessageContent {
    fn from(value: PublicPromptContent) -> Self {
        match value {
            PublicPromptContent::Agent { sender, text } => Self::Agent { sender, text },
            PublicPromptContent::HumanUser { text } => Self::HumanUser { text },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationEnd {
    Completed,
    TimedOut,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExistingConversationPromptRequest {
    pub target: SessionRef,
    pub cwd: std::path::PathBuf,
    pub prompt: ConversationPromptRequest,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExistingConversationPromptResult {
    pub target: SessionRef,
    pub end: ConversationEnd,
    pub updates: Vec<Value>,
    pub permission_required: bool,
    pub result: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreatePromptRequest {
    pub create: ConversationCreateRequest,
    pub prompt: ConversationPromptRequest,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreatePromptResult {
    pub target: SessionRef,
    pub end: ConversationEnd,
    pub updates: Vec<Value>,
    pub permission_required: bool,
    pub result: Option<Value>,
}

pub type ConversationCreatePromptError = crate::OperationError;
pub type ExistingConversationPromptError = crate::OperationError;
