//! Optimistic instruction edits retain history and never pin future execution.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    EventId, InstructionDocument, InstructionId, InstructionText, OperationId, RevisionId,
};
use sqlx::{Connection, Row, SqliteConnection};

pub struct InstructionUpdate {
    pub operation_id: OperationId,
    pub instruction_id: InstructionId,
    pub expected_revision_id: RevisionId,
    pub text: InstructionText,
    pub now_ms: i64,
}

impl AutomationStore {
    pub async fn update_instruction(
        &mut self,
        request: &InstructionUpdate,
    ) -> Result<InstructionDocument, StorageError> {
        if request.now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let canonical = serde_json::to_vec(&(
            &request.instruction_id,
            &request.expected_revision_id,
            &request.text,
        ))
        .map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query("SELECT method_name,canonical_request,operation_status,final_result_json FROM operation_receipts WHERE operation_id=?")
            .bind(request.operation_id.as_str()).fetch_optional(&mut *transaction).await? {
            if row.try_get::<String,_>("method_name")? != "instruction/update" || row.try_get::<Vec<u8>,_>("canonical_request")? != canonical {
                return Err(StorageError::OperationConflict);
            }
            if row.try_get::<String,_>("operation_status")? != "succeeded" { return Err(StorageError::InvalidRecord); }
            let result = row.try_get::<Option<String>,_>("final_result_json")?.ok_or(StorageError::InvalidRecord)?;
            let document = serde_json::from_str(&result).map_err(|_| StorageError::InvalidRecord)?;
            transaction.commit().await?;
            return Ok(document);
        }
        let current = read_current(&mut transaction, &request.instruction_id).await?;
        if current.revision_id != request.expected_revision_id {
            return Err(StorageError::RevisionConflict);
        }
        let updated = InstructionDocument {
            instruction_id: current.instruction_id,
            revision_id: RevisionId::generate(),
            text: request.text.clone(),
            created_at_ms: current.created_at_ms,
            updated_at_ms: request.now_ms.max(current.updated_at_ms),
        };
        sqlx::query("INSERT INTO instruction_revisions(revision_id,instruction_id,instruction_text,source_revision_id,recorded_at_ms) VALUES (?,?,?,NULL,?)")
            .bind(updated.revision_id.as_str()).bind(updated.instruction_id.as_str())
            .bind(updated.text.as_str()).bind(updated.updated_at_ms).execute(&mut *transaction).await?;
        sqlx::query("UPDATE instruction_documents SET current_revision_id=?,instruction_text=?,updated_at_ms=? WHERE instruction_id=?")
            .bind(updated.revision_id.as_str()).bind(updated.text.as_str()).bind(updated.updated_at_ms)
            .bind(updated.instruction_id.as_str()).execute(&mut *transaction).await?;
        let result = serde_json::to_string(&updated).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,'instruction/update',?,?,'succeeded',?,?,NULL,?)")
            .bind(request.operation_id.as_str()).bind(canonical).bind(updated.instruction_id.as_str())
            .bind(r#"{"kind":"local","mutation":"committed"}"#).bind(&result).bind(updated.updated_at_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'instruction',?,'updated',?,?)")
            .bind(EventId::generate().as_str()).bind(updated.instruction_id.as_str()).bind(&result)
            .bind(updated.updated_at_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(updated)
    }
    pub async fn read_instruction(
        &mut self,
        id: &InstructionId,
    ) -> Result<InstructionDocument, StorageError> {
        read_current(&mut self.connection, id).await
    }
}

async fn read_current(
    connection: &mut SqliteConnection,
    id: &InstructionId,
) -> Result<InstructionDocument, StorageError> {
    let row = sqlx::query("SELECT instruction_id,current_revision_id,instruction_text,updated_at_ms,(SELECT MIN(recorded_at_ms) FROM instruction_revisions WHERE instruction_id=d.instruction_id) AS created_at_ms FROM instruction_documents d WHERE instruction_id=?")
        .bind(id.as_str()).fetch_optional(connection).await?.ok_or(StorageError::InstructionNotFound)?;
    Ok(InstructionDocument {
        instruction_id: InstructionId::try_from(row.try_get::<String, _>("instruction_id")?)
            .map_err(|_| StorageError::InvalidRecord)?,
        revision_id: RevisionId::try_from(row.try_get::<String, _>("current_revision_id")?)
            .map_err(|_| StorageError::InvalidRecord)?,
        text: InstructionText::try_from(row.try_get::<String, _>("instruction_text")?)
            .map_err(|_| StorageError::InvalidRecord)?,
        created_at_ms: row.try_get("created_at_ms")?,
        updated_at_ms: row.try_get("updated_at_ms")?,
    })
}
