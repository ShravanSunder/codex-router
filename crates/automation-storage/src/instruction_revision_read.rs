//! Immutable instruction snapshots are independent of the expiring automation event journal.
use crate::{AutomationStore, StorageError};
use agent_automation::{InstructionId, InstructionText, RevisionId};
use sqlx::Row;
pub struct InstructionRevisionRecord {
    pub instruction_id: InstructionId,
    pub revision_id: RevisionId,
    pub text: InstructionText,
    pub source_revision_id: Option<String>,
    pub recorded_at_ms: i64,
}
impl AutomationStore {
    pub async fn read_instruction_revision(
        &mut self,
        revision_id: &RevisionId,
    ) -> Result<InstructionRevisionRecord, StorageError> {
        let row = sqlx::query("SELECT * FROM instruction_revisions WHERE revision_id=?")
            .bind(revision_id.as_str())
            .fetch_optional(&mut self.connection)
            .await?
            .ok_or(StorageError::InstructionNotFound)?;
        Ok(InstructionRevisionRecord {
            instruction_id: row
                .try_get::<String, _>("instruction_id")?
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?,
            revision_id: revision_id.clone(),
            text: row
                .try_get::<String, _>("instruction_text")?
                .try_into()
                .map_err(|_| StorageError::InvalidRecord)?,
            source_revision_id: row.try_get("source_revision_id")?,
            recorded_at_ms: row.try_get("recorded_at_ms")?,
        })
    }
}
