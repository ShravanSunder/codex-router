//! Import validates the complete portable package before any mutation and never allocates native state.
use crate::local_operation_receipts::{self, CompletedLocalOperation, LocalOperation};
use crate::{AutomationStore, ScheduleInspection, StorageError};
use agent_automation::OperationId;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Connection, Row};
pub struct ScheduleImport<'a> {
    pub operation_id: OperationId,
    pub package_utf8: &'a str,
    pub overwrite: bool,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn import_schedule<
        TTarget: Serialize + DeserializeOwned,
        TEndpoint: Serialize + DeserializeOwned,
    >(
        &mut self,
        request: &ScheduleImport<'_>,
    ) -> Result<ScheduleInspection<TTarget, TEndpoint>, StorageError> {
        let package = agent_automation::decode_schedule_package::<TTarget, TEndpoint>(
            request.package_utf8,
            1_048_576,
        )?;
        crate::schedule_repository::validate_definition(&package.definition)?;
        let now = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(request.now_ms)
            .ok_or(StorageError::InvalidRecord)?;
        package.definition.timing.next_due(now, None)?;
        let canonical = serde_json::to_vec(&(request.package_utf8, request.overwrite))
            .map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(result) = local_operation_receipts::replay(
            &mut transaction,
            LocalOperation {
                id: &request.operation_id,
                method: "schedule/import",
                canonical: &canonical,
            },
        )
        .await?
        {
            transaction.commit().await?;
            return Ok(result);
        }
        let existing = sqlx::query(
            "SELECT created_at_ms,definition_json FROM schedule_definitions WHERE schedule_id=?",
        )
        .bind(package.schedule_id.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        if existing.is_some() && !request.overwrite {
            return Err(StorageError::ScheduleImportExists);
        }
        if let Some(existing) = &existing {
            let definition: agent_automation::ScheduleDefinition<TTarget, TEndpoint> =
                serde_json::from_str(&existing.try_get::<String, _>("definition_json")?)
                    .map_err(|_| StorageError::InvalidRecord)?;
            crate::schedule_repository::validate_unchanged_execution_mode(
                &definition.destination,
                &package.definition.destination,
            )?;
        }
        let current = sqlx::query(
            "SELECT instruction_text FROM instruction_documents WHERE instruction_id=?",
        )
        .bind(package.instruction.instruction_id.as_str())
        .fetch_optional(&mut *transaction)
        .await?;
        if let Some(current) = current {
            if current.try_get::<String, _>("instruction_text")?
                != package.instruction.text.as_str()
            {
                let ids:Vec<String>=sqlx::query_scalar("SELECT schedule_id FROM schedule_definitions WHERE instruction_id=? ORDER BY schedule_id").bind(package.instruction.instruction_id.as_str()).fetch_all(&mut *transaction).await?;
                return Err(StorageError::InstructionImportConflict {
                    instruction_id: package.instruction.instruction_id,
                    schedule_ids: ids
                        .into_iter()
                        .map(|id| id.try_into().map_err(|_| StorageError::InvalidRecord))
                        .collect::<Result<_, _>>()?,
                });
            }
        } else {
            let revision = agent_automation::RevisionId::generate();
            sqlx::query("INSERT INTO instruction_documents(instruction_id,current_revision_id,instruction_text,updated_at_ms) VALUES (?,?,?,?)").bind(package.instruction.instruction_id.as_str()).bind(revision.as_str()).bind(package.instruction.text.as_str()).bind(request.now_ms).execute(&mut *transaction).await?;
            sqlx::query("INSERT INTO instruction_revisions(revision_id,instruction_id,instruction_text,source_revision_id,recorded_at_ms) VALUES (?,?,?,?,?)").bind(revision.as_str()).bind(package.instruction.instruction_id.as_str()).bind(package.instruction.text.as_str()).bind(&package.instruction.source_revision_id).bind(request.now_ms).execute(&mut *transaction).await?;
        }
        let change = agent_automation::ChangeId::generate();
        let imported = match package.continuity {
            Some(summary) => agent_automation::ContinuityInput::ImportedSummary {
                text: summary.text.as_str().into(),
                source_run_id: summary.source_run_id,
                source_target: summary.source_target,
                import_operation_id: request.operation_id.clone(),
            },
            None => agent_automation::ContinuityInput::None,
        };
        let definition =
            serde_json::to_string(&package.definition).map_err(|_| StorageError::InvalidRecord)?;
        let continuity =
            serde_json::to_string(&imported).map_err(|_| StorageError::InvalidRecord)?;
        let created = existing
            .as_ref()
            .map(|row| row.try_get::<i64, _>("created_at_ms"))
            .transpose()?
            .unwrap_or(request.now_ms);
        let before = existing
            .as_ref()
            .map(|row| row.try_get::<String, _>("definition_json"))
            .transpose()?
            .map(|text| {
                serde_json::from_str::<serde_json::Value>(&text)
                    .map_err(|_| StorageError::InvalidRecord)
            })
            .transpose()?;
        sqlx::query("INSERT INTO schedule_definitions(schedule_id,change_id,instruction_id,enabled,definition_json,imported_continuity_json,created_at_ms,updated_at_ms) VALUES (?,?,?,0,?,?,?,?) ON CONFLICT(schedule_id) DO UPDATE SET change_id=excluded.change_id,instruction_id=excluded.instruction_id,enabled=0,definition_json=excluded.definition_json,imported_continuity_json=excluded.imported_continuity_json,updated_at_ms=excluded.updated_at_ms")
            .bind(package.schedule_id.as_str()).bind(change.as_str()).bind(package.instruction.instruction_id.as_str()).bind(&definition).bind(continuity).bind(created).bind(request.now_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO schedule_timing_state(schedule_id,applied_change_id,anchor_at_ms,evaluated_through_ms,next_due_at_ms) VALUES (?,?,?,?,NULL) ON CONFLICT(schedule_id) DO UPDATE SET applied_change_id=excluded.applied_change_id,anchor_at_ms=excluded.anchor_at_ms,evaluated_through_ms=excluded.evaluated_through_ms,next_due_at_ms=NULL")
            .bind(package.schedule_id.as_str()).bind(change.as_str()).bind(request.now_ms).bind(request.now_ms).execute(&mut *transaction).await?;
        let event = serde_json::json!({"kind":"scheduleEdit","before":before,"after":package.definition,"sourceChangeId":package.source_change_id});
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'schedule',?,'imported',?,?)").bind(agent_automation::EventId::generate().as_str()).bind(package.schedule_id.as_str()).bind(event.to_string()).bind(request.now_ms).execute(&mut *transaction).await?;
        let record =
            crate::schedule_inspection::read_current(&mut transaction, &package.schedule_id)
                .await?;
        let inventory = crate::run_inventory::load(&mut transaction, &package.schedule_id).await?;
        let result = ScheduleInspection {
            record,
            active_run_id: inventory.occupying,
            waiting_run_id: inventory.waiting,
        };
        let encoded = serde_json::to_string(&result).map_err(|_| StorageError::InvalidRecord)?;
        local_operation_receipts::record(
            &mut transaction,
            CompletedLocalOperation {
                operation: LocalOperation {
                    id: &request.operation_id,
                    method: "schedule/import",
                    canonical: &canonical,
                },
                resource_id: package.schedule_id.as_str(),
                result_json: &encoded,
                now_ms: request.now_ms,
            },
        )
        .await?;
        transaction.commit().await?;
        Ok(result)
    }
}
