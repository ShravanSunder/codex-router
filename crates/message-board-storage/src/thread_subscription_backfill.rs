//! One-time R24 subscription and valid Delivered-position backfill.
use crate::message_records::load_identity;
use crate::storage_support::{BoardTransaction, invalid_record, storage_error};
use crate::thread_delivery_position_writer::write_delivered_position_if_valid;
use crate::thread_subscription_records::{SubscriptionWrite, upsert_subscription};
use crate::thread_subscription_row_decoding::decode_utc_timestamp;
use message_board::*;

struct EligibleBackfillRow {
    reader_key: String,
    root_id: String,
    latest_message_activity: i64,
}

pub(crate) async fn backfill_thread_subscriptions(
    transaction: &mut BoardTransaction<'_>,
) -> Result<(), BoardError> {
    let migration_time: String = sqlx::query_scalar!(
        "SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now') AS \"migration_time!: String\""
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let renewed_at = decode_utc_timestamp("renewedAt", migration_time.as_str())?;
    let eligible_rows = sqlx::query_as!(
        EligibleBackfillRow,
        "SELECT participant.reader_key AS \"reader_key!: String\", \
           participant.root_id AS \"root_id!: String\", \
           MAX(activity.activity_sequence) AS \"latest_message_activity!: i64\" \
         FROM thread_participants participant \
         JOIN board_identities identity ON identity.identity_key=participant.reader_key \
         JOIN board_threads thread ON thread.root_id=participant.root_id \
         JOIN thread_watches watch ON watch.reader_key=participant.reader_key \
           AND watch.root_id=participant.root_id AND watch.active=1 \
         JOIN board_activity activity ON \
           (activity.root_id=participant.root_id AND activity.kind='threadMessageCreated') \
           OR (activity.message_id=participant.root_id AND activity.kind='mainMessageCreated') \
         WHERE participant.closed_at_activity IS NULL \
           AND identity.kind='session' AND thread.state='unresolved' \
         GROUP BY participant.reader_key,participant.root_id",
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;

    for row in eligible_rows {
        let reader = load_identity(transaction, row.reader_key.as_str()).await?;
        let root_message_id = MessageId::try_from(row.root_id).map_err(|_| invalid_record())?;
        let scope = SubscriptionScope::thread(root_message_id.clone());
        let policy = SubscriptionPolicy::defaults_for(&reader);
        let generation = SubscriptionGeneration::new(1).map_err(|_| invalid_record())?;
        upsert_subscription(
            transaction,
            SubscriptionWrite {
                reader_key: &row.reader_key,
                scope: &scope,
                policy: &policy,
                state: SubscriptionState::Active,
                now: renewed_at,
                generation,
                last_outcome: None,
            },
        )
        .await?;

        let latest_message_activity = ActivitySequence::try_from(
            u64::try_from(row.latest_message_activity).map_err(|_| invalid_record())?,
        )
        .map_err(|_| invalid_record())?;
        let _ = write_delivered_position_if_valid(
            transaction,
            row.reader_key.as_str(),
            &root_message_id,
            latest_message_activity,
        )
        .await?;
    }
    Ok(())
}
