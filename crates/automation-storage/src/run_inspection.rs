//! Run inspection validates query projections against the authoritative typed evidence.
use crate::{AutomationStore, StorageError};
use agent_automation::{RunId, RunRecord};
use serde::de::DeserializeOwned;
use sqlx::{Row, SqliteConnection};
impl AutomationStore {
    pub async fn read_run<
        TTarget: DeserializeOwned,
        TEndpoint: DeserializeOwned,
        TGeneration: DeserializeOwned,
        TReceipt: DeserializeOwned,
    >(
        &mut self,
        id: &RunId,
    ) -> Result<RunRecord<TTarget, TEndpoint, TGeneration, TReceipt>, StorageError> {
        read_current(&mut self.connection, id).await
    }
}
pub(crate) async fn read_current<
    TTarget: DeserializeOwned,
    TEndpoint: DeserializeOwned,
    TGeneration: DeserializeOwned,
    TReceipt: DeserializeOwned,
>(
    connection: &mut SqliteConnection,
    id: &RunId,
) -> Result<RunRecord<TTarget, TEndpoint, TGeneration, TReceipt>, StorageError> {
    let row = sqlx::query("SELECT * FROM workflow_runs WHERE run_id=?")
        .bind(id.as_str())
        .fetch_optional(connection)
        .await?
        .ok_or(StorageError::InvalidRecord)?;
    fn decode<TValue: DeserializeOwned>(value: String) -> Result<TValue, StorageError> {
        serde_json::from_str(&value).map_err(|_| StorageError::InvalidRecord)
    }
    let evidence: agent_automation::RunExecutionEvidence<TTarget, TGeneration, TReceipt> =
        decode(row.try_get("execution_evidence_json")?)?;
    let start: Option<i64> = row.try_get("execution_started_at_ms")?;
    let deadline: Option<i64> = row.try_get("execution_deadline_at_ms")?;
    let timeout: Option<i64> = row.try_get("effective_timeout_seconds")?;
    if evidence
        .timing
        .as_ref()
        .map(|time| time.dispatch_started_at_ms)
        != start
        || evidence.timing.as_ref().map(|time| time.deadline_at_ms) != deadline
        || evidence
            .timing
            .as_ref()
            .map(|time| i64::from(time.effective_timeout_seconds))
            != timeout
    {
        return Err(StorageError::InvalidRecord);
    }
    Ok(RunRecord {
        run_id: id.clone(),
        schedule_id: row
            .try_get::<String, _>("schedule_id")?
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?,
        due_at_ms: row.try_get("due_at_ms")?,
        phase: serde_json::from_value(serde_json::Value::String(row.try_get("run_status")?))
            .map_err(|_| StorageError::InvalidRecord)?,
        inputs: row
            .try_get::<Option<String>, _>("captured_inputs_json")?
            .map(decode)
            .transpose()?,
        thread_binding_id: row
            .try_get::<Option<String>, _>("thread_binding_id")?
            .map(|id| id.try_into().map_err(|_| StorageError::InvalidRecord))
            .transpose()?,
        native_turn_id: row.try_get("native_turn_id")?,
        evidence,
        worker_outcome: row
            .try_get::<Option<String>, _>("worker_outcome_json")?
            .map(decode)
            .transpose()?,
        summary_attempt: row
            .try_get::<Option<String>, _>("summary_attempt_json")?
            .map(decode)
            .transpose()?,
        summary_text: row.try_get("summary_text")?,
        summary_source: row
            .try_get::<Option<String>, _>("summary_source_json")?
            .map(decode)
            .transpose()?,
        completed_at_ms: row.try_get("completed_at_ms")?,
    })
}
