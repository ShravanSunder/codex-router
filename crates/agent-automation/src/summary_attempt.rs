//! A summary is a separate bounded attempt on the same Run, never another workflow occurrence.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SummaryPhase {
    Preparing,
    Running,
    Stopping,
    Uncertain,
    Completed,
    Failed,
    Skipped,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryAttempt<TTarget, TGeneration> {
    pub attempt_id: crate::AttemptId,
    pub source_target: TTarget,
    #[serde(
        alias = "sourceTurnId",
        deserialize_with = "crate::summary_source_reference::deserialize_stored_source_reference"
    )]
    pub source_reference: crate::SummarySourceReference,
    pub target: Option<TTarget>,
    pub native_turn_id: Option<String>,
    pub effective_timeout_seconds: u32,
    pub started_at_ms: i64,
    pub deadline_at_ms: i64,
    pub phase: SummaryPhase,
    pub effects: crate::NativeEffectEvidence<TTarget, TGeneration>,
    pub explanation: Option<String>,
}
