//! Attempt history always discloses its retention coverage, including empty pages.
use crate::{
    AttemptId, CessationEvidence, DeliveryEvidence, DeliveryId, ObservationTimestamp, PageLimit,
    PositiveSeconds, RunId, SessionRef,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptInspection {
    pub attempt_id: AttemptId,
    pub delivery_id: DeliveryId,
    pub attempt_number: u32,
    pub began_at: ObservationTimestamp,
    #[serde(deserialize_with = "Option::deserialize")]
    pub ended_at: Option<ObservationTimestamp>,
    pub evidence: DeliveryEvidence,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SummaryInspectionState {
    Preparing,
    Running,
    Stopping,
    Uncertain,
    Completed,
    Failed,
    Skipped,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryInspection {
    pub summary_attempt_id: AttemptId,
    pub run_id: RunId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub target: Option<SessionRef>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub native_turn_id: Option<String>,
    pub effective_timeout_seconds: PositiveSeconds,
    #[serde(deserialize_with = "Option::deserialize")]
    pub started_at: Option<ObservationTimestamp>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub deadline_at: Option<ObservationTimestamp>,
    pub state: SummaryInspectionState,
    pub cessation: CessationEvidence,
    pub retry_eligible: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub explanation: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub summary_run_id: Option<RunId>,
}
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EarlierAttempts {
    MayBeUnavailable,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptHistoryCoverage {
    pub history_from: ObservationTimestamp,
    pub as_of: ObservationTimestamp,
    pub earlier_attempts: EarlierAttempts,
    pub latest_attempt_included: bool,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttemptHistoryPage<TRecord> {
    pub records: Vec<TRecord>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub next_cursor: Option<String>,
    pub coverage: AttemptHistoryCoverage,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryAttemptsRequest {
    pub delivery_id: DeliveryId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
#[derive(Clone, Debug, JsonSchema, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunSummariesRequest {
    pub run_id: RunId,
    #[serde(deserialize_with = "Option::deserialize")]
    pub cursor: Option<String>,
    pub limit: PageLimit,
}
