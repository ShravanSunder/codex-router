//! Thirty-day bounded cleanup for retained automation payloads and receipts.
use crate::{AutomationStore, StorageError};

const MAX_AUTOMATION_PRUNE_BATCH: u32 = 500;
const RETENTION_DAYS: i64 = 30;
const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

impl AutomationStore {
    /// Deletes settled wake payload rows that no wake still references as pending.
    pub async fn prune_settled_mailbox_deliveries(
        &mut self,
        now_ms: i64,
        batch_size: u32,
    ) -> Result<u64, StorageError> {
        validate_batch_size(batch_size)?;
        let cutoff_ms = retention_cutoff_ms(now_ms)?;
        let result = sqlx::query(
            "DELETE FROM mailbox_deliveries WHERE rowid IN (SELECT delivery.rowid FROM mailbox_deliveries AS delivery WHERE delivery.created_at_ms < ? AND delivery.delivery_status IN ('accepted','failed','discarded') AND NOT EXISTS (SELECT 1 FROM wakeup_definitions AS wake WHERE wake.pending_delivery_id=delivery.delivery_id) ORDER BY delivery.created_at_ms,delivery.delivery_id LIMIT ?)",
        )
        .bind(cutoff_ms)
        .bind(i64::from(batch_size))
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected())
    }

    /// Deletes whole replay receipts after the idempotency retention window.
    pub async fn prune_operation_receipts(
        &mut self,
        now_ms: i64,
        batch_size: u32,
    ) -> Result<u64, StorageError> {
        validate_batch_size(batch_size)?;
        let cutoff_ms = retention_cutoff_ms(now_ms)?;
        let result = sqlx::query(
            "DELETE FROM operation_receipts WHERE rowid IN (SELECT rowid FROM operation_receipts WHERE committed_at_ms < ? ORDER BY committed_at_ms,operation_id LIMIT ?)",
        )
        .bind(cutoff_ms)
        .bind(i64::from(batch_size))
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected())
    }
}

fn retention_cutoff_ms(now_ms: i64) -> Result<i64, StorageError> {
    let duration_ms = RETENTION_DAYS
        .checked_mul(MILLIS_PER_DAY)
        .ok_or(StorageError::InvalidRecord)?;
    now_ms
        .checked_sub(duration_ms)
        .ok_or(StorageError::InvalidRecord)
}

fn validate_batch_size(batch_size: u32) -> Result<(), StorageError> {
    if (1..=MAX_AUTOMATION_PRUNE_BATCH).contains(&batch_size) {
        Ok(())
    } else {
        Err(StorageError::InvalidRecord)
    }
}
