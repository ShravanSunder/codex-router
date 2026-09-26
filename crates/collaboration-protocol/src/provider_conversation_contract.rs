//! Provider-neutral external ACP conversation and operation contracts.
use crate::{
    CodexGeneration, EndpointRef, MessageContent, MessageText, NonEmptyText, ObservationTimestamp,
    PositiveSeconds, RouterAccess, SessionRef,
};
use agent_automation::OperationId;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderKind {
    ClaudeCode,
    Cursor,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRuntimeIdentity {
    pub provider: ProviderKind,
    pub runtime_name: NonEmptyText,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_version: Option<NonEmptyText>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
pub enum ProviderTransport {
    #[serde(rename = "stdioAcp")]
    StdioAcp,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderBindingId(String);

impl TryFrom<String> for ProviderBindingId {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=256).contains(&value.len()) && !value.contains('\0') {
            Ok(Self(value))
        } else {
            Err("provider binding ID requires 1 to 256 UTF-8 bytes without NUL")
        }
    }
}

impl From<ProviderBindingId> for String {
    fn from(value: ProviderBindingId) -> Self {
        value.0
    }
}

impl JsonSchema for ProviderBindingId {
    fn schema_name() -> Cow<'static, str> {
        "ProviderBindingId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": 256,
            "pattern": "^[^\\u0000]+$",
            "x-maxUtf8Bytes": 256
        })
    }
}

#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, JsonSchema, Serialize, Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapabilityName {
    Create,
    Prompt,
    Load,
    Cancel,
    Permissions,
    CollaborationMcp,
    ExactOperationReconciliation,
    CallerDetach,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapabilityStatus {
    Supported,
    Unsupported,
    Unverified,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderCapabilityEvidence {
    Advertised,
    Observed,
    RouterQualified,
    NotAvailable,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderCapability {
    pub name: ProviderCapabilityName,
    pub status: ProviderCapabilityStatus,
    pub evidence: ProviderCapabilityEvidence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Vec<ProviderCapability>", into = "Vec<ProviderCapability>")]
pub struct ProviderCapabilities(Vec<ProviderCapability>);

impl TryFrom<Vec<ProviderCapability>> for ProviderCapabilities {
    type Error = &'static str;

    fn try_from(capabilities: Vec<ProviderCapability>) -> Result<Self, Self::Error> {
        let names = capabilities
            .iter()
            .map(|capability| capability.name)
            .collect::<BTreeSet<_>>();
        if capabilities.is_empty() || capabilities.len() > 32 {
            return Err("provider capabilities require 1 to 32 entries");
        }
        if names.len() != capabilities.len() {
            return Err("provider capability names must be unique");
        }
        Ok(Self(capabilities))
    }
}

impl From<ProviderCapabilities> for Vec<ProviderCapability> {
    fn from(capabilities: ProviderCapabilities) -> Self {
        capabilities.0
    }
}

impl ProviderCapabilities {
    #[must_use]
    pub fn as_slice(&self) -> &[ProviderCapability] {
        &self.0
    }
}

impl JsonSchema for ProviderCapabilities {
    fn schema_name() -> Cow<'static, str> {
        "ProviderCapabilities".into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let item_schema = generator.subschema_for::<ProviderCapability>();
        json_schema!({
            "type": "array",
            "minItems": 1,
            "maxItems": 32,
            "items": item_schema
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderBindingIdentity {
    pub endpoint: EndpointRef,
    pub binding_id: ProviderBindingId,
    pub runtime: ProviderRuntimeIdentity,
    pub transport: ProviderTransport,
    pub generation: CodexGeneration,
    pub capabilities: ProviderCapabilities,
}

/// The binding that admitted a conversation operation. External provider identity
/// remains distinct from the Codex ACP listener that owns a native session.
#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConversationBindingIdentity {
    ExternalProvider {
        binding: ProviderBindingIdentity,
    },
    #[serde(rename_all = "camelCase")]
    CodexAcp {
        endpoint: EndpointRef,
        listener_path: NonEmptyText,
        generation: CodexGeneration,
    },
}

impl ConversationBindingIdentity {
    #[must_use]
    pub fn external_provider(&self) -> Option<&ProviderBindingIdentity> {
        match self {
            Self::ExternalProvider { binding } => Some(binding),
            Self::CodexAcp { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderWorkingDirectory(String);

impl TryFrom<String> for ProviderWorkingDirectory {
    type Error = &'static str;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if (1..=4096).contains(&value.len())
            && !value.contains('\0')
            && std::path::Path::new(&value).is_absolute()
        {
            Ok(Self(value))
        } else {
            Err(
                "provider working directory must be an absolute path of 1 to 4096 bytes without NUL",
            )
        }
    }
}

impl From<ProviderWorkingDirectory> for String {
    fn from(value: ProviderWorkingDirectory) -> Self {
        value.0
    }
}

impl JsonSchema for ProviderWorkingDirectory {
    fn schema_name() -> Cow<'static, str> {
        "ProviderWorkingDirectory".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": 4096,
            "pattern": "^/[^\\u0000]*$",
            "x-maxUtf8Bytes": 4096
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRequestedPolicy {
    pub access: RouterAccess,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderSettingsMappingStatus {
    Verified,
    Unverified,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderAuthenticationState {
    Unverified,
    Authenticated,
    AuthenticationRequired,
    AuthenticationFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderPermissionOutcome {
    Allowed,
    Rejected,
    Unanswered,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EffectiveProviderSettings {
    pub requested_policy: ProviderRequestedPolicy,
    pub mapping_status: ProviderSettingsMappingStatus,
    pub authentication: ProviderAuthenticationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_permission_mode: Option<NonEmptyText>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_outcome: Option<ProviderPermissionOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderOperationKind {
    ConversationCreate,
    ConversationLoad,
    ConversationPrompt,
    ConversationCancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderOperationStage {
    Admitted,
    MayHaveDispatched,
    Terminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationOperationFailureStage {
    Validation,
    Binding,
    Admission,
    Dispatch,
    Settlement,
    Reconciliation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderOperationEffect {
    None,
    Applied,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderReconciliationState {
    Unresolved,
    Confirmed,
    NotReconcilable,
}

#[derive(Clone, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ConversationOperationQueueState {
    RouterQueued,
    NotSubmitted { reason: String },
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationSnapshot {
    pub operation_id: OperationId,
    pub operation: ProviderOperationKind,
    pub binding: ConversationBindingIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<SessionRef>,
    pub stage: ProviderOperationStage,
    pub effect: ProviderOperationEffect,
    pub reconciliation: ProviderReconciliationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_stop_reason: Option<ProviderPromptStopReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_state: Option<ConversationOperationQueueState>,
    pub admitted_at: ObservationTimestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_at: Option<ObservationTimestamp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationAdmissionState {
    Admitted,
    Existing,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationSubmission {
    pub admission: ConversationAdmissionState,
    pub operation: ConversationOperationSnapshot,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCreateRequest {
    pub operation_id: OperationId,
    pub endpoint: EndpointRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CodexGeneration>,
    pub working_directory: ProviderWorkingDirectory,
    pub created_by: SessionRef,
    pub approver: SessionRef,
    pub requested_policy: ProviderRequestedPolicy,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationLoadRequest {
    pub operation_id: OperationId,
    pub target: SessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CodexGeneration>,
    pub working_directory: ProviderWorkingDirectory,
    pub requested_by: SessionRef,
    pub approver: SessionRef,
    pub requested_policy: ProviderRequestedPolicy,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationPromptRequest {
    pub operation_id: OperationId,
    pub target: SessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CodexGeneration>,
    pub requested_by: SessionRef,
    pub approver: SessionRef,
    pub prompt: MessageContent,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationCancelRequest {
    pub operation_id: OperationId,
    pub target_operation_id: OperationId,
    pub target: SessionRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CodexGeneration>,
    pub requested_by: SessionRef,
    pub approver: SessionRef,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationShowRequest {
    pub operation_id: OperationId,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationWaitRequest {
    pub operation_id: OperationId,
    pub timeout_seconds: PositiveSeconds,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationReconcileRequest {
    pub operation_id: OperationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderPromptStopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationOperationSettlement {
    Created {
        target: SessionRef,
        effective_settings: EffectiveProviderSettings,
    },
    Loaded {
        target: SessionRef,
        effective_settings: EffectiveProviderSettings,
    },
    PromptCompleted {
        target: SessionRef,
        stop_reason: ProviderPromptStopReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<MessageText>,
    },
    CancelRequested {
        target: SessionRef,
        target_operation_id: OperationId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationOutputUnavailableReason {
    HostRestarted,
    NotRetained,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConversationOperationWaitOutput {
    Pending,
    Available {
        settlement: ConversationOperationSettlement,
    },
    OutputUnavailable {
        reason: ConversationOutputUnavailableReason,
    },
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationWaitResult {
    pub operation: ConversationOperationSnapshot,
    pub output: ConversationOperationWaitOutput,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConversationOperationFailureKind {
    InvalidRequest,
    UnsupportedCapability,
    AuthenticationRequired,
    PermissionRejected,
    Busy,
    NotFound,
    ProviderSessionNotFound,
    StaleGeneration,
    Unavailable,
    ProviderRejected,
    ProtocolViolation,
    OutcomeUnknown,
}

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConversationOperationFailure {
    pub kind: ConversationOperationFailureKind,
    pub stage: ConversationOperationFailureStage,
    pub effect: ProviderOperationEffect,
    pub message: NonEmptyText,
    pub operation_id: OperationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_code: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<SessionRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<EndpointRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<crate::EndpointAvailability>,
}
