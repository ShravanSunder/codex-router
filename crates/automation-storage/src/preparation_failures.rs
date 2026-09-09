//! Failed and uncertain preparation retain evidence; neither path silently repeats a native call.
use crate::{AutomationStore, StorageError};
use agent_automation::{OperationId, ScheduleId};
use serde::Serialize;
pub enum PreparationFailureDisposition {
    Failed,
    Uncertain,
}
pub struct PreparationFailureRecord<TState, TError> {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub disposition: PreparationFailureDisposition,
    pub evidence: TState,
    pub error: TError,
}
impl AutomationStore {
    pub async fn record_preparation_failure<TState: Serialize, TError: Serialize>(
        &mut self,
        request: PreparationFailureRecord<TState, TError>,
    ) -> Result<(), StorageError> {
        let status = match request.disposition {
            PreparationFailureDisposition::Failed => "failed",
            PreparationFailureDisposition::Uncertain => "uncertain",
        };
        let rows=sqlx::query("UPDATE operation_receipts SET operation_status=?,effect_evidence_json=?,final_error_json=? WHERE operation_id=? AND resource_id=? AND method_name='schedule/prepare' AND operation_status IN ('admitted','inProgress','uncertain')")
            .bind(status).bind(serde_json::to_string(&request.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(serde_json::to_string(&request.error).map_err(|_|StorageError::InvalidRecord)?).bind(request.operation_id.as_str()).bind(request.schedule_id.as_str()).execute(&mut self.connection).await?.rows_affected();
        if rows == 1 {
            Ok(())
        } else {
            Err(StorageError::OperationConflict)
        }
    }
}
