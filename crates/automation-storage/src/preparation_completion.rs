//! Native preparation completion binds the actual thread and updates only future configuration atomically.
use crate::{AutomationStore, ScheduleInspection, StorageError, ThreadBindingClaim};
use agent_automation::{ChangeId, ExecutionDestination, OperationId, ScheduleId, ScheduleRecord};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct PreparationIntent<TState> {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub evidence: TState,
}
pub struct PreparedThread<TTarget, TState> {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub expected_change_id: ChangeId,
    pub target: TTarget,
    pub service_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub cwd: String,
    pub evidence: TState,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn record_preparation_intent<TState: Serialize>(
        &mut self,
        request: &PreparationIntent<TState>,
    ) -> Result<bool, StorageError> {
        let evidence =
            serde_json::to_string(&request.evidence).map_err(|_| StorageError::InvalidRecord)?;
        let changed=sqlx::query("UPDATE operation_receipts SET operation_status='inProgress',effect_evidence_json=? WHERE operation_id=? AND resource_id=? AND method_name='schedule/prepare' AND operation_status='admitted'")
            .bind(evidence).bind(request.operation_id.as_str()).bind(request.schedule_id.as_str()).execute(&mut self.connection).await?.rows_affected();
        Ok(changed == 1)
    }
    pub async fn complete_thread_preparation<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
        TState: Serialize,
    >(
        &mut self,
        request: PreparedThread<TTarget, TState>,
    ) -> Result<ScheduleInspection<TTarget, TEndpoint>, StorageError> {
        if request.now_ms < 0 || !std::path::Path::new(&request.cwd).is_absolute() {
            return Err(StorageError::InvalidRecord);
        }
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let operation=sqlx::query("SELECT operation_status,resource_id FROM operation_receipts WHERE operation_id=? AND method_name='schedule/prepare'").bind(request.operation_id.as_str()).fetch_optional(&mut *transaction).await?.ok_or(StorageError::InvalidRecord)?;
        if operation.try_get::<String, _>("operation_status")? != "inProgress"
            || operation.try_get::<String, _>("resource_id")? != request.schedule_id.as_str()
        {
            return Err(StorageError::OperationConflict);
        }
        let mut record: ScheduleRecord<TTarget, TEndpoint> =
            crate::schedule_inspection::read_current(&mut transaction, &request.schedule_id)
                .await?;
        if record.change_id != request.expected_change_id {
            return Err(StorageError::ScheduleChangeConflict);
        }
        crate::schedule_repository::validate_unchanged_execution_mode(
            &record.definition.destination,
            &ExecutionDestination::Unprepared,
        )?;
        crate::thread_binding_repository::claim_in_transaction(
            &mut transaction,
            &ThreadBindingClaim {
                schedule_id: request.schedule_id.clone(),
                service_id: request.service_id,
                endpoint_id: request.endpoint_id,
                thread_id: request.thread_id,
                now_ms: request.now_ms,
            },
        )
        .await?;
        let before =
            serde_json::to_value(&record.definition).map_err(|_| StorageError::InvalidRecord)?;
        record.definition.destination = ExecutionDestination::OwnedThread {
            target: request.target,
            cwd: request.cwd,
        };
        record.change_id = ChangeId::generate();
        record.updated_at_ms = request.now_ms;
        let definition =
            serde_json::to_string(&record.definition).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE schedule_definitions SET change_id=?,definition_json=?,updated_at_ms=? WHERE schedule_id=?").bind(record.change_id.as_str()).bind(definition).bind(request.now_ms).bind(request.schedule_id.as_str()).execute(&mut *transaction).await?;
        sqlx::query("UPDATE schedule_timing_state SET applied_change_id=? WHERE schedule_id=?")
            .bind(record.change_id.as_str())
            .bind(request.schedule_id.as_str())
            .execute(&mut *transaction)
            .await?;
        let event =
            serde_json::json!({"kind":"scheduleEdit","before":before,"after":record.definition});
        let inventory = crate::run_inventory::load(&mut transaction, &request.schedule_id).await?;
        let result = ScheduleInspection {
            record,
            active_run_id: inventory.occupying,
            waiting_run_id: inventory.waiting,
        };
        sqlx::query("UPDATE operation_receipts SET operation_status='succeeded',effect_evidence_json=?,final_result_json=? WHERE operation_id=?")
            .bind(serde_json::to_string(&request.evidence).map_err(|_|StorageError::InvalidRecord)?).bind(serde_json::to_string(&result).map_err(|_|StorageError::InvalidRecord)?).bind(request.operation_id.as_str()).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'schedule',?,'prepared',?,?)").bind(agent_automation::EventId::generate().as_str()).bind(request.schedule_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(result)
    }
}
