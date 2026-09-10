//! Schedule edits change future work; admitted Run inputs remain immutable.
use crate::local_operation_receipts::{self, CompletedLocalOperation, LocalOperation};
use crate::{AutomationStore, StorageError};
use agent_automation::{ChangeId, OperationId, ScheduleDefinition, ScheduleId, ScheduleRecord};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::Connection;
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScheduleEdit<TTarget, TEndpoint> {
    Replace {
        expected_change_id: ChangeId,
        definition: ScheduleDefinition<TTarget, TEndpoint>,
    },
    SetEnabled {
        enabled: bool,
    },
}
pub struct ScheduleMutation<TTarget, TEndpoint> {
    pub operation_id: OperationId,
    pub schedule_id: ScheduleId,
    pub edit: ScheduleEdit<TTarget, TEndpoint>,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn mutate_schedule<
        TTarget: Clone + Serialize + DeserializeOwned,
        TEndpoint: Clone + Serialize + DeserializeOwned,
    >(
        &mut self,
        request: &ScheduleMutation<TTarget, TEndpoint>,
    ) -> Result<crate::ScheduleInspection<TTarget, TEndpoint>, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let method = match request.edit {
            ScheduleEdit::Replace { .. } => "schedule/update",
            ScheduleEdit::SetEnabled { enabled: true } => "schedule/enable",
            ScheduleEdit::SetEnabled { enabled: false } => "schedule/disable",
        };
        let canonical = serde_json::to_vec(&(&request.schedule_id, &request.edit))
            .map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(record) = local_operation_receipts::replay(
            &mut transaction,
            LocalOperation {
                id: &request.operation_id,
                method,
                canonical: &canonical,
            },
        )
        .await?
        {
            transaction.commit().await?;
            return Ok(record);
        }
        let mut current: ScheduleRecord<TTarget, TEndpoint> =
            crate::schedule_inspection::read_current(&mut transaction, &request.schedule_id)
                .await?;
        let before = current.definition.clone();
        match &request.edit {
            ScheduleEdit::Replace {
                expected_change_id,
                definition,
            } => {
                if *expected_change_id != current.change_id {
                    return Err(StorageError::ScheduleChangeConflict);
                }
                crate::schedule_repository::validate_unchanged_execution_mode(
                    &current.definition.destination,
                    &definition.destination,
                )?;
                current.definition = definition.clone();
            }
            ScheduleEdit::SetEnabled { enabled } => current.definition.enabled = *enabled,
        }
        crate::schedule_repository::validate_definition(&current.definition)?;
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(current.anchor_at_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(request.now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let timing_changed = before.timing != current.definition.timing
            || before.enabled != current.definition.enabled;
        if timing_changed {
            let calculated = current.definition.timing.next_due(anchor, Some(now))?;
            current.next_due_at_ms = if current.definition.enabled {
                calculated.map(|at| at.timestamp_millis())
            } else {
                None
            };
        }
        let instruction: Option<String> = sqlx::query_scalar(
            "SELECT instruction_id FROM instruction_documents WHERE instruction_id=?",
        )
        .bind(current.definition.instruction_id.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        if instruction.is_none() {
            return Err(StorageError::InstructionNotFound);
        }
        current.change_id = ChangeId::generate();
        current.updated_at_ms = request.now_ms;
        let definition =
            serde_json::to_string(&current.definition).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("UPDATE schedule_definitions SET change_id=?,instruction_id=?,enabled=?,definition_json=?,updated_at_ms=? WHERE schedule_id=?")
            .bind(current.change_id.as_str()).bind(current.definition.instruction_id.as_str()).bind(current.definition.enabled).bind(definition).bind(request.now_ms).bind(request.schedule_id.as_str()).execute(&mut *transaction).await?;
        sqlx::query("UPDATE schedule_timing_state SET applied_change_id=?,next_due_at_ms=?,evaluated_through_ms=CASE WHEN ? THEN MAX(evaluated_through_ms,?) ELSE evaluated_through_ms END WHERE schedule_id=?")
            .bind(current.change_id.as_str()).bind(current.next_due_at_ms).bind(timing_changed).bind(request.now_ms).bind(request.schedule_id.as_str()).execute(&mut *transaction).await?;
        let event =
            serde_json::json!({"kind":"scheduleEdit","before":before,"after":current.definition});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'schedule',?,'edited',?,?)")
            .bind(agent_automation::EventId::generate().as_str()).bind(request.schedule_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        let inventory = crate::run_inventory::load(&mut transaction, &request.schedule_id).await?;
        let current = crate::ScheduleInspection {
            record: current,
            active_run_id: inventory.occupying,
            waiting_run_id: inventory.waiting,
        };
        let result = serde_json::to_string(&current).map_err(|_| StorageError::InvalidRecord)?;
        local_operation_receipts::record(
            &mut transaction,
            CompletedLocalOperation {
                operation: LocalOperation {
                    id: &request.operation_id,
                    method,
                    canonical: &canonical,
                },
                resource_id: request.schedule_id.as_str(),
                result_json: &result,
                now_ms: request.now_ms,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(current)
    }
}
