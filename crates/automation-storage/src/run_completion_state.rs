//! Only cessation of the exact recorded turn can finish work or enter required summarization.
use crate::{AutomationStore, StorageError};
use agent_automation::{RunId, WorkerOutcome};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
pub struct RunCompletion {
    pub run_id: RunId,
    pub native_turn_id: String,
    pub outcome: WorkerOutcome,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn complete_run_turn<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
        TGeneration: Serialize + DeserializeOwned,
        TReceipt: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: RunCompletion,
    ) -> Result<bool, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let mut record =
            crate::run_inspection::read_current::<TTarget, TEndpoint, TGeneration, TReceipt>(
                &mut transaction,
                &request.run_id,
            )
            .await?;
        if !matches!(
            record.phase,
            agent_automation::RunPhase::Executing
                | agent_automation::RunPhase::Stopping
                | agent_automation::RunPhase::Uncertain
        ) || record.native_turn_id.as_deref() != Some(request.native_turn_id.as_str())
        {
            transaction.commit().await?;
            return Ok(false);
        }
        let inputs = record.inputs.as_ref().ok_or(StorageError::InvalidRecord)?;
        let requires_summary = matches!(
            inputs.execution_configuration.destination,
            agent_automation::ExecutionDestination::FreshEachRun { .. }
        );
        let phase = if requires_summary {
            "summaryRequired"
        } else {
            "finished"
        };
        record.evidence.native.cessation = agent_automation::CessationEvidence::Confirmed;
        let completed = request.now_ms.max(
            record
                .evidence
                .timing
                .as_ref()
                .map_or(0, |time| time.dispatch_started_at_ms),
        );
        sqlx::query("UPDATE workflow_runs SET run_status=?,worker_outcome_json=?,execution_evidence_json=?,completed_at_ms=? WHERE run_id=?")
            .bind(phase).bind(serde_json::to_string(&request.outcome).map_err(|_|StorageError::InvalidRecord)?).bind(serde_json::to_string(&record.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(if requires_summary{None}else{Some(completed)}).bind(request.run_id.as_str()).execute(&mut *transaction).await?;
        let event = serde_json::json!({"kind":"stateChange","before":record.phase,"after":phase});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'run',?,'workerStopped',?,?)")
            .bind(agent_automation::EventId::generate().as_str()).bind(request.run_id.as_str()).bind(event.to_string()).bind(completed).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(true)
    }
}
