//! Shared validation of a Reader's Delivered position and pending Thread message count.
use crate::message_records::require_thread;
use crate::storage_support::{
    BoardTransaction, invalid_record, storage_error, validate_stored_boundary,
};
use message_board::*;

#[derive(Clone, Debug)]
pub(super) struct SubscriptionRootSelection {
    pub root_message_id: MessageId,
}

struct ThreadDeliveredBoundary {
    effective_delivered_position: i64,
}

struct StoredPendingRootRange {
    pending_count: i64,
    first_pending_sequence: Option<i64>,
    through_sequence: Option<i64>,
}

async fn load_thread_delivered_boundary(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    latest: i64,
) -> Result<Option<ThreadDeliveredBoundary>, BoardError> {
    let resource = ResourceIdentity::Thread {
        root_message_id: root_message_id.clone(),
    };
    let row = sqlx::query!(
        "SELECT watch.starts_after_activity,position.delivered_through, \
           CASE WHEN position.delivered_through IS NULL THEN 1 ELSE EXISTS( \
             SELECT 1 FROM board_activity activity \
             WHERE activity.activity_sequence=position.delivered_through \
               AND ((activity.root_id=watch.root_id AND activity.kind='threadMessageCreated') \
                 OR (activity.message_id=watch.root_id AND activity.kind='mainMessageCreated'))) END AS valid_scope \
         FROM thread_watches watch \
         LEFT JOIN thread_delivery_positions position \
           ON position.reader_key=watch.reader_key AND position.root_id=watch.root_id \
         WHERE watch.reader_key=? AND watch.root_id=? AND watch.active=1",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    validate_stored_boundary(row.starts_after_activity, 0, latest, resource.clone())?;
    let effective_delivered_position = match row.delivered_through {
        Some(delivered_position) => {
            validate_stored_boundary(
                delivered_position,
                row.starts_after_activity,
                latest,
                resource.clone(),
            )?;
            if row.valid_scope != 1 {
                return Err(BoardError::invalid_record(resource));
            }
            delivered_position
        }
        None => {
            let effective = row.starts_after_activity;
            validate_stored_boundary(effective, row.starts_after_activity, latest, resource)?;
            effective
        }
    };
    Ok(Some(ThreadDeliveredBoundary {
        effective_delivered_position,
    }))
}

pub(crate) async fn pending_message_count(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    latest: i64,
) -> Result<i64, BoardError> {
    let boundary =
        load_thread_delivered_boundary(transaction, reader_key, root_message_id, latest).await?;
    let Some(boundary) = boundary else {
        return Ok(0);
    };
    sqlx::query_scalar!(
        "SELECT count(*) FROM board_activity activity \
         WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') \
           OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
           AND activity.message_id IS NOT NULL AND activity.actor_key<>? \
           AND activity.activity_sequence>?",
        root_message_id.as_str(),
        root_message_id.as_str(),
        reader_key,
        boundary.effective_delivered_position,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)
}

/// Select bounded root locators from validated Reader Delivered positions.
pub(super) async fn select_pending_root_notices(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    roots: &[SubscriptionRootSelection],
    latest: i64,
) -> Result<Vec<PendingRootNotice>, BoardError> {
    let mut notices = Vec::with_capacity(roots.len().min(MAX_SUBSCRIPTION_NOTICE_ROOTS));
    for root in roots {
        if notices.len() == MAX_SUBSCRIPTION_NOTICE_ROOTS {
            break;
        }
        let Some(boundary) =
            load_thread_delivered_boundary(transaction, reader_key, &root.root_message_id, latest)
                .await?
        else {
            continue;
        };
        let resource = ResourceIdentity::Thread {
            root_message_id: root.root_message_id.clone(),
        };
        let range = sqlx::query_as!(
            StoredPendingRootRange,
            "SELECT COUNT(*) AS \"pending_count!: i64\", \
               MIN(activity.activity_sequence) AS first_pending_sequence, \
               MAX(activity.activity_sequence) AS through_sequence \
             FROM board_activity activity \
             WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') \
               OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
               AND activity.message_id IS NOT NULL AND activity.actor_key<>? \
               AND activity.activity_sequence>?",
            root.root_message_id.as_str(),
            root.root_message_id.as_str(),
            reader_key,
            boundary.effective_delivered_position,
        )
        .fetch_one(&mut **transaction)
        .await
        .map_err(storage_error)?;
        if range.pending_count == 0 {
            continue;
        }
        let first_pending = range.first_pending_sequence.ok_or_else(invalid_record)?;
        let through = range.through_sequence.ok_or_else(invalid_record)?;
        validate_stored_boundary(
            first_pending,
            boundary.effective_delivered_position,
            latest,
            resource.clone(),
        )?;
        validate_stored_boundary(through, first_pending, latest, resource.clone())?;

        let location = require_thread(transaction, &root.root_message_id).await?;
        let notice = PendingRootNotice::new(
            root.root_message_id.clone(),
            location.topic_id,
            crate::message_records::activity_sequence(first_pending)?,
            crate::message_records::activity_sequence(through)?,
            u64::try_from(range.pending_count).map_err(|_| invalid_record())?,
        )
        .map_err(|_| invalid_record())?;
        notices.push(notice);
    }
    Ok(notices)
}
