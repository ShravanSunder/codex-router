//! Persist Delivered positions only after session delivery reaches its reader.
use crate::BoardStore;
use crate::participant_records::advance_participant_last_seen;
use crate::storage_support::{
    current_activity_sequence, ensure_identity, invalid_record, storage_error,
    validate_stored_boundary,
};
use message_board::*;
use sqlx::Connection;

impl BoardStore {
    /// Persist the delivered position after a listen batch has reached its reader.
    pub async fn record_thread_listen_batch_delivery(
        &mut self,
        context: &ThreadListenContext,
        batch_set: &ThreadListenBatchSet,
    ) -> Result<(), BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, &context.reader).await?;
        let latest = current_activity_sequence(&mut transaction).await?;
        for batch in &batch_set.batches {
            let delivered_value =
                i64::try_from(batch.delivered_through.get()).map_err(|_| invalid_record())?;
            let watch_start = sqlx::query_scalar!(
                "SELECT starts_after_activity FROM thread_watches WHERE reader_key=? AND root_id=? AND active=1",
                reader_key,
                batch.root_message_id.as_str(),
            )
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| {
                BoardError::invalid_record(ResourceIdentity::Thread {
                    root_message_id: batch.root_message_id.clone(),
                })
            })?;
            validate_stored_boundary(
                delivered_value,
                watch_start,
                latest,
                ResourceIdentity::Thread {
                    root_message_id: batch.root_message_id.clone(),
                },
            )?;
            sqlx::query!(
                "INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through) VALUES(?,?,?) \
                 ON CONFLICT(reader_key,root_id) DO UPDATE SET delivered_through=excluded.delivered_through",
                reader_key,
                batch.root_message_id.as_str(),
                delivered_value,
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            advance_participant_last_seen(
                &mut transaction,
                &reader_key,
                &batch.root_message_id,
                delivered_value,
            )
            .await?;
        }
        transaction.commit().await.map_err(storage_error)
    }
}
