//! Instruction mutations are committed with history and a replay receipt.
use crate::{AutomationStore, StorageError};
use agent_automation::{
    EventId, InstructionDocument, InstructionId, InstructionText, OperationId, RevisionId,
};
use sqlx::{Connection, Row};

impl AutomationStore {
    pub async fn create_instruction(
        &mut self,
        operation_id: &OperationId,
        text: &InstructionText,
        now_ms: i64,
    ) -> Result<InstructionDocument, StorageError> {
        if now_ms < 0 {
            return Err(StorageError::InvalidRecord);
        }
        let request = serde_json::to_vec(text).map_err(|_| StorageError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(row) = sqlx::query("SELECT method_name,canonical_request,operation_status,final_result_json FROM operation_receipts WHERE operation_id=?")
            .bind(operation_id.as_str()).fetch_optional(&mut *transaction).await? {
            let method: String = row.try_get("method_name")?;
            let existing: Vec<u8> = row.try_get("canonical_request")?;
            if method != "instruction/create" || existing != request {
                return Err(StorageError::OperationConflict);
            }
            let status: String = row.try_get("operation_status")?;
            if status != "succeeded" { return Err(StorageError::InvalidRecord); }
            let result: Option<String> = row.try_get("final_result_json")?;
            let result = result.ok_or(StorageError::InvalidRecord)?;
            let document = serde_json::from_str(&result).map_err(|_| StorageError::InvalidRecord)?;
            transaction.commit().await?;
            return Ok(document);
        }
        let document = InstructionDocument {
            instruction_id: InstructionId::generate(),
            revision_id: RevisionId::generate(),
            text: text.clone(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        };
        let result = serde_json::to_string(&document).map_err(|_| StorageError::InvalidRecord)?;
        sqlx::query("INSERT INTO instruction_documents(instruction_id,current_revision_id,instruction_text,updated_at_ms) VALUES (?,?,?,?)")
            .bind(document.instruction_id.as_str()).bind(document.revision_id.as_str())
            .bind(text.as_str()).bind(now_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO instruction_revisions(revision_id,instruction_id,instruction_text,source_revision_id,recorded_at_ms) VALUES (?,?,?,NULL,?)")
            .bind(document.revision_id.as_str()).bind(document.instruction_id.as_str())
            .bind(text.as_str()).bind(now_ms).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO operation_receipts(operation_id,method_name,canonical_request,resource_id,operation_status,effect_evidence_json,final_result_json,final_error_json,committed_at_ms) VALUES (?,'instruction/create',?,?,'succeeded',?, ?,NULL,?)")
            .bind(operation_id.as_str()).bind(request).bind(document.instruction_id.as_str())
            .bind(r#"{"kind":"local","mutation":"committed"}"#).bind(&result).bind(now_ms)
            .execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO automation_events(event_id,subject_kind,subject_id,event_kind,event_body_json,recorded_at_ms) VALUES (?,'instruction',?,'created',?,?)")
            .bind(EventId::generate().as_str()).bind(document.instruction_id.as_str())
            .bind(&result).bind(now_ms).execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(document)
    }
}
