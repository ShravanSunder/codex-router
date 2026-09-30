//! Guarded monotonic writes to the per-reader, per-root Delivered cursor.
use crate::storage_support::{BoardTransaction, invalid_record, storage_error};
use message_board::{ActivitySequence, BoardError, MessageId};

pub(crate) async fn latest_message_activity_for_root(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
) -> Result<Option<ActivitySequence>, BoardError> {
    let latest = sqlx::query_scalar!(
        "SELECT MAX(activity_sequence) AS latest_message_activity FROM board_activity \
         WHERE (root_id=? AND kind='threadMessageCreated') \
           OR (message_id=? AND kind='mainMessageCreated')",
        root_message_id.as_str(),
        root_message_id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    latest
        .map(|sequence| {
            ActivitySequence::try_from(u64::try_from(sequence).map_err(|_| invalid_record())?)
                .map_err(|_| invalid_record())
        })
        .transpose()
}

/// Advances Delivered only at a valid message activity inside the active root Watch.
pub(crate) async fn write_delivered_position_if_valid(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    through: ActivitySequence,
) -> Result<bool, BoardError> {
    let through = i64::try_from(through.get()).map_err(|_| invalid_record())?;
    let result = sqlx::query!(
        "WITH candidate(reader_key,root_id,delivered_through) AS (VALUES(?,?,?)) \
         INSERT INTO thread_delivery_positions(reader_key,root_id,delivered_through) \
         SELECT candidate.reader_key,candidate.root_id,candidate.delivered_through \
         FROM candidate JOIN thread_watches watch \
           ON watch.reader_key=candidate.reader_key AND watch.root_id=candidate.root_id \
         WHERE watch.active=1 \
           AND candidate.delivered_through>=watch.starts_after_activity \
           AND candidate.delivered_through>COALESCE( \
             (SELECT position.delivered_through FROM thread_delivery_positions position \
              WHERE position.reader_key=candidate.reader_key AND position.root_id=candidate.root_id), \
             watch.starts_after_activity) \
           AND EXISTS(SELECT 1 FROM board_activity activity \
             WHERE activity.activity_sequence=candidate.delivered_through \
               AND ((activity.root_id=candidate.root_id AND activity.kind='threadMessageCreated') \
                 OR (activity.message_id=candidate.root_id AND activity.kind='mainMessageCreated'))) \
         ON CONFLICT(reader_key,root_id) DO UPDATE SET \
           delivered_through=MAX(thread_delivery_positions.delivered_through,excluded.delivered_through)",
        reader_key,
        root_message_id.as_str(),
        through,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(result.rows_affected() > 0)
}
