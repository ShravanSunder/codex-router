//! Control-owned native operation parameters and results; native payload schemas stay upstream.
use crate::{
    AcceptedResumeEffect, CodexGeneration, MessageContent, MessageDelivery, MessageInputKind,
    MessageRepresentation, NonEmptyText, SessionRef,
};
use serde::{Deserialize, Serialize};

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInspectParams {
    pub target: SessionRef,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInspectResult {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    /// Validated against the advertised native Thread definition before publication.
    #[schemars(schema_with = "crate::native_schema_references::thread_schema")]
    pub thread: serde_json::Value,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInterruptParams {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub turn_id: NonEmptyText,
}
#[derive(schemars::JsonSchema, Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum NativeInterruptKind {
    #[serde(rename = "interruptCompleted")]
    InterruptCompleted,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeInterruptResult {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub turn_id: NonEmptyText,
    pub kind: NativeInterruptKind,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSendParams {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub message: MessageContent,
    #[serde(default)]
    pub delivery: MessageDelivery,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_user_message_id: Option<NonEmptyText>,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum NativeSendAcceptance {
    NativeInputAccepted {
        operation: NativeInputOperation,
        disposition: NativeInputDisposition,
        #[serde(rename = "turnId")]
        turn_id: NonEmptyText,
    },
    QueueAccepted {
        #[serde(rename = "submissionId")]
        submission_id: NonEmptyText,
    },
    SteerAccepted {
        #[serde(rename = "turnId")]
        turn_id: NonEmptyText,
        #[serde(rename = "submissionId", skip_serializing_if = "Option::is_none")]
        submission_id: Option<NonEmptyText>,
    },
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeInputOperation {
    TurnStart,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeInputDisposition {
    StartedOrSteered,
}
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeSendReceipt {
    pub target: SessionRef,
    pub generation: CodexGeneration,
    pub input_kind: MessageInputKind,
    pub representation: MessageRepresentation,
    pub client_user_message_id: NonEmptyText,
    pub resume_effect: AcceptedResumeEffect,
    pub acceptance: NativeSendAcceptance,
}
