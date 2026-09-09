//! A completed summary releases the same Run only after its exact current attempt is confirmed stopped.
use crate::{AutomationStore, StorageError};
use agent_automation::{AttemptId, InstructionText, RunId};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct SummaryCompletion {
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub native_turn_id: String,
    pub text: InstructionText,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn complete_summary<
        TTarget: Serialize + DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: SummaryCompletion,
    ) -> Result<bool, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let row=sqlx::query("SELECT summary_attempt_json FROM workflow_runs WHERE run_id=? AND run_status IN ('summaryRunning','summaryBlocked') AND worker_outcome_json IS NOT NULL").bind(request.run_id.as_str()).fetch_optional(&mut *transaction).await?;
        let Some(row) = row else {
            transaction.commit().await?;
            return Ok(false);
        };
        let raw: String = row.try_get("summary_attempt_json")?;
        let mut attempt: agent_automation::SummaryAttempt<TTarget, TGeneration> =
            serde_json::from_str(&raw).map_err(|_| StorageError::InvalidRecord)?;
        if attempt.attempt_id != request.attempt_id
            || attempt.native_turn_id.as_deref() != Some(request.native_turn_id.as_str())
            || !matches!(
                attempt.phase,
                agent_automation::SummaryPhase::Running | agent_automation::SummaryPhase::Uncertain
            )
        {
            transaction.commit().await?;
            return Ok(false);
        }
        attempt.phase = agent_automation::SummaryPhase::Completed;
        attempt.effects.cessation = agent_automation::CessationEvidence::Confirmed;
        attempt.effects.submission = agent_automation::SubmissionEffect::Accepted;
        let encoded = serde_json::to_string(&attempt).map_err(|_| StorageError::InvalidRecord)?;
        let source = agent_automation::SummarySource::Completed {
            source_target: attempt.source_target,
            source_turn_id: attempt.source_turn_id,
            summary_attempt_id: attempt.attempt_id,
        };
        sqlx::query("UPDATE workflow_runs SET run_status='finished',summary_attempt_json=?,summary_text=?,summary_source_json=?,completed_at_ms=? WHERE run_id=?")
            .bind(encoded).bind(request.text.as_str()).bind(serde_json::to_string(&source).map_err(|_|StorageError::InvalidRecord)?).bind(request.now_ms.max(attempt.started_at_ms)).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        let event =
            serde_json::json!({"kind":"stateChange","before":"summaryRunning","after":"finished"});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,'summaryCompleted',?,?)").bind(agent_automation::EventId::generate().as_str()).bind(request.run_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}
