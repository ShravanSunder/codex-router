//! Native side-effect evidence is data, not permission to replay an uncertain submission.
use crate::AttemptId;
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreparationEffect {
    NotRequested,
    NotDispatched,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubmissionEffect {
    NotDispatched,
    Dispatching,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CessationEvidence {
    NotApplicable,
    Unconfirmed,
    Confirmed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeEffectEvidence<TTarget, TGeneration> {
    pub target: Option<TTarget>,
    pub generation: Option<TGeneration>,
    pub client_user_message_id: Option<String>,
    pub native_turn_id: Option<String>,
    pub native_submission_id: Option<String>,
    pub allocation: PreparationEffect,
    pub resume: PreparationEffect,
    pub submission: SubmissionEffect,
    pub cessation: CessationEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum AttemptOutcome {
    InProgress,
    Accepted,
    KnownNotSubmitted { reason: String, retryable: bool },
    Unknown { reason: String },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryAttempt<TTarget, TGeneration> {
    pub attempt_id: AttemptId,
    pub attempt_number: u32,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    /// Pause/cancel prevents retry even if this attempt resolves after resume.
    pub discard_on_non_submission: bool,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub outcome: AttemptOutcome,
}
