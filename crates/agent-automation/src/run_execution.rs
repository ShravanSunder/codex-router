//! Run execution evidence freezes time budgets only when dispatch becomes eligible.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunExecutionEvidence<TTarget, TGeneration, TReceipt> {
    pub native: crate::NativeEffectEvidence<TTarget, TGeneration>,
    pub timing: Option<ExecutionTiming>,
    pub acceptance: Option<TReceipt>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionTiming {
    pub dispatch_started_at_ms: i64,
    pub effective_timeout_seconds: u32,
    pub deadline_at_ms: i64,
}
impl ExecutionTiming {
    pub fn start(now_ms: i64, seconds: u32) -> Option<Self> {
        if now_ms < 0 || !(1..=31_536_000).contains(&seconds) {
            return None;
        }
        Some(Self {
            dispatch_started_at_ms: now_ms,
            effective_timeout_seconds: seconds,
            deadline_at_ms: now_ms.checked_add(i64::from(seconds) * 1000)?,
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum WorkerOutcome {
    Completed { explanation: Option<String> },
    Failed { explanation: Option<String> },
    Interrupted { explanation: Option<String> },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunRecord<TTarget, TEndpoint, TGeneration, TReceipt> {
    pub run_id: crate::RunId,
    pub schedule_id: crate::ScheduleId,
    pub due_at_ms: i64,
    pub phase: crate::RunPhase,
    pub inputs: Option<crate::CapturedRunInputs<TTarget, TEndpoint>>,
    pub thread_binding_id: Option<crate::ThreadBindingId>,
    pub native_turn_id: Option<String>,
    pub evidence: RunExecutionEvidence<TTarget, TGeneration, TReceipt>,
    pub worker_outcome: Option<WorkerOutcome>,
    pub summary_attempt: Option<crate::SummaryAttempt<TTarget, TGeneration>>,
    pub summary_text: Option<String>,
    pub summary_source: Option<crate::SummarySource<TTarget>>,
    pub completed_at_ms: Option<i64>,
}
