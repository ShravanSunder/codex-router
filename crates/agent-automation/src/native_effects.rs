//! Native side-effect evidence is data, not permission to replay an uncertain submission.
use crate::AttemptId;
use serde::{Deserialize, Deserializer, Serialize};
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
    RouterQueued,
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
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryAttempt<TTarget, TGeneration> {
    pub attempt_id: AttemptId,
    pub attempt_number: u32,
    pub started_at_ms: i64,
    pub completed_at_ms: Option<i64>,
    /// Pause/cancel prevents retry even if this attempt resolves after resume.
    pub discard_on_non_submission: bool,
    /// None means claimed, with no route selected and no client I/O yet.
    pub effects: Option<crate::RouteEffectEvidence<TTarget, TGeneration>>,
    pub outcome: AttemptOutcome,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeliveryAttemptFields<TTarget, TGeneration> {
    attempt_id: AttemptId,
    attempt_number: u32,
    started_at_ms: i64,
    completed_at_ms: Option<i64>,
    discard_on_non_submission: bool,
    effects: Option<crate::RouteEffectEvidence<TTarget, TGeneration>>,
    outcome: AttemptOutcome,
}

impl<'de, TTarget, TGeneration> Deserialize<'de> for DeliveryAttempt<TTarget, TGeneration>
where
    TTarget: Deserialize<'de>,
    TGeneration: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let fields = DeliveryAttemptFields::deserialize(deserializer)?;
        let completed = fields.completed_at_ms.is_some();
        let outcome_completed = !matches!(&fields.outcome, AttemptOutcome::InProgress);
        if fields.attempt_number == 0
            || fields.started_at_ms < 0
            || fields
                .completed_at_ms
                .is_some_and(|at| at < fields.started_at_ms)
            || completed != outcome_completed
            || (fields.effects.is_none()
                && matches!(
                    &fields.outcome,
                    AttemptOutcome::Accepted | AttemptOutcome::Unknown { .. }
                ))
        {
            return Err(serde::de::Error::custom("invalid delivery attempt state"));
        }
        Ok(Self {
            attempt_id: fields.attempt_id,
            attempt_number: fields.attempt_number,
            started_at_ms: fields.started_at_ms,
            completed_at_ms: fields.completed_at_ms,
            discard_on_non_submission: fields.discard_on_non_submission,
            effects: fields.effects,
            outcome: fields.outcome,
        })
    }
}
