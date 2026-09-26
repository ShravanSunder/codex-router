//! Client-neutral completion and pending evidence for conversation work.
use collaboration_protocol::{
    ConversationOutputUnavailableReason, EffectiveProviderSettings, MessageText, OperationId,
    SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationOperationResult {
    Completed {
        target: SessionRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        operation_id: Option<OperationId>,
        settlement: ConversationSettlement,
    },
    Pending {
        operation_id: OperationId,
        target: SessionRef,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationCreatePromptOutcome {
    CreatePending {
        operation_id: OperationId,
    },
    Prompt {
        create_operation_id: OperationId,
        prompt: Box<ConversationOperationResult>,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationSettlement {
    pub target: SessionRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<ConversationStopReason>,
    pub detail: ConversationSettlementDetail,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationStopReason {
    Completed,
    TimedOut,
    Cancelled,
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationSettlementDetail {
    CodexPrompt {
        updates: Vec<Value>,
        permission_required: bool,
        result: Option<Value>,
    },
    CodexLoad,
    ProviderPrompt {
        output: ProviderPromptOutput,
    },
    ProviderLoad {
        output: ProviderLoadOutput,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderPromptOutput {
    Available {
        text: Option<MessageText>,
    },
    Unavailable {
        reason: ConversationOutputUnavailableReason,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ProviderLoadOutput {
    Available {
        effective_settings: EffectiveProviderSettings,
    },
    Unavailable {
        reason: ConversationOutputUnavailableReason,
    },
}
