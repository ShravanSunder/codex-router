//! Schedule definitions and their timer state are created atomically but remain separate records.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    ChangeId, ContinuityInput, EventId, ExecutionDestination, OperationId, ScheduleDefinition,
    ScheduleId, ScheduleRecord,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};

pub struct ScheduleCreate<TTarget, TEndpoint> {
    pub operation_id: OperationId,
    pub definition: ScheduleDefinition<TTarget, TEndpoint>,
    pub imported_continuity: ContinuityInput<TTarget>,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn create_schedule<TTarget, TEndpoint>(
        &mut self,
        request: &ScheduleCreate<TTarget, TEndpoint>,
    ) -> Result<ScheduleRecord<TTarget, TEndpoint>, StorageError>
    where
        TTarget: Clone + Serialize + DeserializeOwned,
        TEndpoint: Clone + Serialize + DeserializeOwned,
    {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let anchor = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(request.now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        let calculated = request.definition.timing.next_due(anchor, None)?;
        validate_definition(&request.definition)?;
        if !matches!(
            request.imported_continuity,
            ContinuityInput::None | ContinuityInput::ImportedSummary { .. }
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let canonical = serde_json::to_vec(&(&request.definition, &request.imported_continuity))
            .map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row)=sqlx::query("SELECT method_name,canonical_request,operation_status,final_result_json FROM operation_receipts WHERE operation_id=?").bind(request.operation_id.as_str()).fetch_optional(&mut *transaction).await? {
            if row.try_get::<String,_>("method_name")? != "schedule/create" || row.try_get::<Vec<u8>,_>("canonical_request")? != canonical { return Err(StorageError::OperationConflict); }
            if row.try_get::<String,_>("operation_status")? != "succeeded" { return Err(StorageError::InvalidRecord); }
            let result=row.try_get::<Option<String>,_>("final_result_json")?.ok_or(StorageError::InvalidRecord)?;
            let record=serde_json::from_str(&result).map_err(|_|StorageError::InvalidRecord)?;
            transaction.commit().await?;return Ok(record);
        }
        let instruction: Option<String> = sqlx::query_scalar(
            "SELECT instruction_id FROM instruction_documents WHERE instruction_id=?",
        )
        .bind(request.definition.instruction_id.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        if instruction.is_none() {
            return Err(StorageError::InstructionNotFound);
        }
        let record = ScheduleRecord {
            schedule_id: ScheduleId::generate(),
            change_id: ChangeId::generate(),
            definition: request.definition.clone(),
            imported_continuity: request.imported_continuity.clone(),
            anchor_at_ms: request.now_ms,
            next_due_at_ms: if request.definition.enabled {
                calculated.map(|value| value.timestamp_millis())
            } else {
                None
            },
            created_at_ms: request.now_ms,
            updated_at_ms: request.now_ms,
        };
        let definition =
            serde_json::to_string(&record.definition).map_err(|_| StorageError::InvalidRecord)?;
        let continuity = serde_json::to_string(&record.imported_continuity)
            .map_err(|_| StorageError::InvalidRecord)?;
        let result = serde_json::to_string(&record).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("INSERT INTO schedule_definitions(schedule_id,change_id,instruction_id,enabled,definition_json,imported_continuity_json,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?)")
            .bind(record.schedule_id.as_str()).bind(record.change_id.as_str()).bind(record.definition.instruction_id.as_str()).bind(record.definition.enabled)
            .bind(definition).bind(continuity).bind(request.now_ms).bind(request.now_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO schedule_timing_state(schedule_id,applied_change_id,anchor_at_ms,evaluated_through_ms,next_due_at_ms) VALUES (?,?,?,?,?)")
            .bind(record.schedule_id.as_str()).bind(record.change_id.as_str()).bind(request.now_ms).bind(request.now_ms).bind(record.next_due_at_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,'schedule/create',?,?,'succeeded',?,?,NULL,?)")
            .bind(request.operation_id.as_str()).bind(canonical).bind(record.schedule_id.as_str()).bind(r#"{"kind":"local","mutation":"committed"}"#).bind(&result).bind(request.now_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'schedule',?,'created',?,?)")
            .bind(EventId::generate().as_str()).bind(record.schedule_id.as_str()).bind(&result).bind(request.now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(record)
    }
}

pub(crate) fn validate_definition<TTarget, TEndpoint>(
    definition: &ScheduleDefinition<TTarget, TEndpoint>,
) -> Result<(), StorageError> {
    match &definition.destination {
        ExecutionDestination::Unprepared if definition.enabled => {
            return Err(StorageError::InvalidSchedule {
                field: "enabled",
                reason: "unprepared schedules must be disabled",
            });
        }
        ExecutionDestination::OwnedThread { cwd, .. }
        | ExecutionDestination::FreshEachRun { cwd, .. }
            if !std::path::Path::new(cwd).is_absolute() =>
        {
            return Err(StorageError::InvalidSchedule {
                field: "destination.cwd",
                reason: "workspace path must be absolute",
            });
        }
        _ => {}
    }
    if definition
        .execution_timeout_seconds
        .is_some_and(|seconds| !(1..=31_536_000).contains(&seconds))
    {
        return Err(StorageError::InvalidSchedule {
            field: "executionTimeoutSeconds",
            reason: "must be between 1 and 31536000 seconds",
        });
    }
    Ok(())
}
