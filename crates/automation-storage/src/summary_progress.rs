//! Latest summary effects stay on their Run; stale attempts cannot overwrite a replacement.
use crate::{AutomationStore, StorageError};
use agent_automation::{AttemptId, NativeEffectEvidence, RunId, SummaryAttempt, SummaryPhase};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct SummaryProgress<TTarget, TGeneration> {
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub phase: SummaryPhase,
    pub effects: NativeEffectEvidence<TTarget, TGeneration>,
    pub target: Option<TTarget>,
    pub native_turn_id: Option<String>,
    pub explanation: Option<String>,
}
impl AutomationStore {
    pub async fn record_summary_progress<
        TTarget: Serialize + DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: SummaryProgress<TTarget, TGeneration>,
    ) -> Result<bool, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT summary_attempt_json FROM workflow_runs WHERE run_id=? AND run_status IN ('summaryRunning','summaryBlocked')").bind(request.run_id.as_str()).fetch_optional(&mut *transaction).await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(false);
        };
        let raw: String = row.try_get("summary_attempt_json")?;
        let mut attempt: SummaryAttempt<TTarget, TGeneration> =
            serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
        if attempt.attempt_id != request.attempt_id
            || matches!(
                attempt.phase,
                SummaryPhase::Completed | SummaryPhase::Skipped
            )
        {
            transaction.commit().await?;
            return Ok(false);
        }
        if matches!(
            request.phase,
            SummaryPhase::Completed | SummaryPhase::Skipped
        ) {
            return Err(StorageError::InvalidRecord);
        }
        attempt.phase = request.phase;
        attempt.effects = request.effects;
        attempt.target = request.target;
        attempt.native_turn_id = request.native_turn_id;
        attempt.explanation = request.explanation;
        sqlx::query("UPDATE workflow_runs SET run_status=?,summary_attempt_json=? WHERE run_id=?")
            .bind(
                if matches!(
                    attempt.phase,
                    SummaryPhase::Failed | SummaryPhase::Uncertain
                ) {
                    "summaryBlocked"
                } else {
                    "summaryRunning"
                },
            )
            .bind(serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?)
            .bind(request.run_id.as_str())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        Ok(true)
    }
}
