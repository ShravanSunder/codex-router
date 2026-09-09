//! Exact native addresses belong exclusively to one schedule; binding identity is local UUIDv7.
use crate::{AutomationStore, StorageError};
use agent_automation::{ScheduleId, ThreadBindingId};
use sqlx::{Connection, Row, SqliteConnection};
pub struct ThreadBindingClaim {
    pub schedule_id: ScheduleId,
    pub service_id: String,
    pub endpoint_id: String,
    pub thread_id: String,
    pub now_ms: i64,
}
impl AutomationStore {
    pub async fn claim_thread_binding(
        &mut self,
        claim: &ThreadBindingClaim,
    ) -> Result<ThreadBindingId, StorageError> {
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let binding = claim_in_transaction(&mut transaction, claim).await?;
        transaction.commit().await?;
        Ok(binding)
    }
}
pub(crate) async fn claim_in_transaction(
    connection: &mut SqliteConnection,
    claim: &ThreadBindingClaim,
) -> Result<ThreadBindingId, StorageError> {
    if claim.now_ms < 0
        || [&claim.service_id, &claim.endpoint_id, &claim.thread_id]
            .iter()
            .any(|value| value.is_empty() || value.contains('\0') || value.len() > 4096)
    {
        return Err(StorageError::InvalidRecord);
    }
    let existing=sqlx::query("SELECT thread_binding_id,schedule_id FROM thread_bindings WHERE service_id=? AND endpoint_id=? AND thread_id=?").bind(&claim.service_id).bind(&claim.endpoint_id).bind(&claim.thread_id).fetch_optional(&mut *connection).await?;
    if let Some(existing) = existing {
        let owner: ScheduleId = existing
            .try_get::<String, _>("schedule_id")?
            .try_into()
            .map_err(|_| StorageError::InvalidRecord)?;
        if owner != claim.schedule_id {
            return Err(StorageError::ThreadOwnershipConflict { schedule_id: owner });
        }
        return existing
            .try_get::<String, _>("thread_binding_id")?
            .try_into()
            .map_err(|_| StorageError::InvalidRecord);
    }
    let binding = ThreadBindingId::generate();
    sqlx::query("INSERT INTO thread_bindings(thread_binding_id,schedule_id,service_id,endpoint_id,thread_id,claimed_at_ms) VALUES (?,?,?,?,?,?)")
        .bind(binding.as_str()).bind(claim.schedule_id.as_str()).bind(&claim.service_id).bind(&claim.endpoint_id).bind(&claim.thread_id).bind(claim.now_ms).execute(connection).await?;
    Ok(binding)
}
