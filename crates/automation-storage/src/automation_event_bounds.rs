//! Retention boundaries use two calendar months, independent of whether cleanup has already run.
use crate::{AutomationStore, StorageError};
impl AutomationStore {
    /// Removes a bounded batch strictly before the UTC calendar cutoff; owner records are never pruned.
    pub async fn prune_automation_events(
        &mut self,
        now_ms: i64,
        batch_limit: u32,
    ) -> Result<u64, StorageError> {
        if !(1..=1000).contains(&batch_limit) {
            return Err(StorageError::InvalidRecord);
        }
        let cutoff = retention_cutoff(now_ms)?;
        let removed = sqlx::query("DELETE FROM automation_events WHERE event_sequence IN (SELECT event_sequence FROM automation_events WHERE recorded_at_ms < ? ORDER BY event_sequence LIMIT ?)")
            .bind(cutoff).bind(batch_limit).execute(&mut self.connection).await?;
        Ok(removed.rows_affected())
    }
    pub async fn automation_event_floor(
        &mut self,
        now_ms: i64,
    ) -> Result<Option<(i64, i64)>, StorageError> {
        let cutoff = retention_cutoff(now_ms)?;
        let first=sqlx::query_as("SELECT event_sequence,recorded_at_ms FROM automation_events WHERE recorded_at_ms>=? ORDER BY event_sequence LIMIT 1").bind(cutoff).fetch_optional(&mut self.connection).await?;
        Ok(first)
    }
}
pub(crate) fn retention_cutoff(now_ms: i64) -> Result<i64, StorageError> {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .and_then(|now| now.checked_sub_months(chrono::Months::new(2)))
        .map(|cutoff| cutoff.timestamp_millis())
        .ok_or(StorageError::InvalidRecord)
}
