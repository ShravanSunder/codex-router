//! Shared read-only inspection failures and exact parent-scoped collection requests.
use crate::{
    InstructionId, LocalMutationEvidence, ObservationTimestamp, PageLimit, RevisionId, ScheduleId,
    WakeupId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunListRequest {
    pub schedule_id: ScheduleId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryListRequest {
    #[serde(deserialize_with = "Option::deserialize")]
    pub wakeup_id: Option<WakeupId>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionListRequest {
    pub instruction_id: InstructionId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionRecord {
    pub revision_id: RevisionId,
    pub instruction_id: InstructionId,
    pub recorded_at: ObservationTimestamp,
    #[serde(deserialize_with = "Option::deserialize")]
    pub source_revision_id: Option<String>,
    pub text: String,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationInspectionFailureKind {
    InvalidField,
    ResourceNotFound,
    AutomationUnavailable,
    InvalidRecord,
    HistoryExpired,
    Overloaded,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationInspectionStage {
    Validation,
    Inspection,
    Reconciliation,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutomationInspectionNextAction {
    CorrectRequest,
    VerifyResourceAddress,
    RefreshCurrentState,
    RetryLater,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationInspectionFailure {
    pub kind: AutomationInspectionFailureKind,
    pub stage: AutomationInspectionStage,
    pub message: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub resource_id: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub field: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub constraint: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub earliest_retained_cursor: Option<String>,
    pub effects: LocalMutationEvidence,
    pub next_action: AutomationInspectionNextAction,
}
