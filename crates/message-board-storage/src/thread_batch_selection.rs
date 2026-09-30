//! Shared bounded Thread batch selection for listening and subscriptions.
use crate::message_records::{load_message, require_thread};
use crate::storage_support::{
    BoardTransaction, invalid_record, storage_error, validate_stored_boundary,
};
use message_board::*;

pub(crate) const THREAD_BATCH_MESSAGE_LIMIT: usize = 100;

#[derive(Clone, Debug)]
pub(crate) struct ThreadBatchSelectionRoot {
    pub root_message_id: MessageId,
    pub initial_delivered_position: Option<ActivitySequence>,
}

pub(crate) struct ThreadBatchBoundary {
    pub stored_delivered_position: Option<i64>,
    pub effective_delivered_position: i64,
}

pub(crate) struct ThreadBatchSelection {
    pub batches: Vec<ThreadBatch>,
}

struct StoredPendingRootRange {
    pending_count: i64,
    first_pending_sequence: Option<i64>,
    through_sequence: Option<i64>,
}

pub(crate) async fn load_thread_batch_boundary(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    initial_delivered_position: Option<ActivitySequence>,
    latest: i64,
) -> Result<Option<ThreadBatchBoundary>, BoardError> {
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
            let initial = initial_delivered_position
                .map(|position| i64::try_from(position.get()).map_err(|_| invalid_record()))
                .transpose()?
                .unwrap_or(row.starts_after_activity);
            let effective = initial.max(row.starts_after_activity);
            validate_stored_boundary(effective, row.starts_after_activity, latest, resource)?;
            effective
        }
    };
    Ok(Some(ThreadBatchBoundary {
        stored_delivered_position: row.delivered_through,
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
        load_thread_batch_boundary(transaction, reader_key, root_message_id, None, latest).await?;
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

/// Select bounded per-root locators while sharing listen's validated Delivered boundary.
pub(crate) async fn select_pending_root_notices(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    roots: &[ThreadBatchSelectionRoot],
    latest: i64,
) -> Result<Vec<PendingRootNotice>, BoardError> {
    let mut notices = Vec::with_capacity(roots.len().min(MAX_SUBSCRIPTION_NOTICE_ROOTS));
    for root in roots {
        if notices.len() == MAX_SUBSCRIPTION_NOTICE_ROOTS {
            break;
        }
        let Some(boundary) = load_thread_batch_boundary(
            transaction,
            reader_key,
            &root.root_message_id,
            root.initial_delivered_position,
            latest,
        )
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

pub(crate) async fn select_thread_batches<FEnvelopeSize>(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    roots: &[ThreadBatchSelectionRoot],
    latest: i64,
    maximum_batch_bytes: usize,
    encoded_envelope_size: FEnvelopeSize,
) -> Result<ThreadBatchSelection, BoardError>
where
    FEnvelopeSize: Fn(&[ThreadBatch]) -> Result<usize, BoardError>,
{
    let mut batches = Vec::new();
    let mut remaining_message_limit = THREAD_BATCH_MESSAGE_LIMIT;
    for root in roots {
        let available_messages = remaining_message_limit;
        let Some(boundary) = load_thread_batch_boundary(
            transaction,
            reader_key,
            &root.root_message_id,
            root.initial_delivered_position,
            latest,
        )
        .await?
        else {
            continue;
        };
        let fetch_limit = i64::try_from(remaining_message_limit)
            .map_err(|_| invalid_record())?
            .checked_add(1)
            .ok_or_else(invalid_record)?;
        let message_ids = sqlx::query_scalar!(
            "SELECT activity.message_id AS \"message_id!: String\" FROM board_activity activity \
             WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
               AND activity.message_id IS NOT NULL AND activity.actor_key<>? \
               AND activity.activity_sequence>? \
             ORDER BY activity.activity_sequence ASC LIMIT ?",
            root.root_message_id.as_str(),
            root.root_message_id.as_str(),
            reader_key,
            boundary.effective_delivered_position,
            fetch_limit,
        )
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage_error)?;
        if message_ids.is_empty() {
            continue;
        }

        let mut selected_messages = Vec::new();
        let mut delivered_through;
        let mut stopped_on_budget = false;
        for message_id in message_ids.iter().take(remaining_message_limit) {
            let message_id =
                MessageId::try_from(message_id.clone()).map_err(|_| invalid_record())?;
            let message = load_message(transaction, &message_id).await?;
            selected_messages.push(ThreadBatchMessage {
                activity_sequence: message.activity_sequence,
                message_id: message.message_id,
                actor: message.actor,
                text: message.text,
            });
            delivered_through = selected_messages
                .last()
                .ok_or_else(invalid_record)?
                .activity_sequence;
            let mut candidate_batches = batches.clone();
            candidate_batches.push(ThreadBatch {
                root_message_id: root.root_message_id.clone(),
                delivered_through,
                messages: selected_messages.clone(),
            });
            if encoded_envelope_size(&candidate_batches)? > maximum_batch_bytes {
                selected_messages.pop();
                if selected_messages.is_empty() && batches.is_empty() {
                    return Err(BoardError::board_unavailable());
                }
                stopped_on_budget = true;
                break;
            }
            remaining_message_limit -= 1;
            if remaining_message_limit == 0 {
                stopped_on_budget = true;
                break;
            }
        }

        if !selected_messages.is_empty() {
            delivered_through = selected_messages
                .last()
                .ok_or_else(invalid_record)?
                .activity_sequence;
            batches.push(ThreadBatch {
                root_message_id: root.root_message_id.clone(),
                delivered_through,
                messages: selected_messages,
            });
        }

        let root_has_remaining_messages =
            stopped_on_budget || message_ids.len() > available_messages;
        if root_has_remaining_messages {
            break;
        }
    }

    Ok(ThreadBatchSelection { batches })
}
