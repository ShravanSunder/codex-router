//! Checked SQLx row reads and domain decoding for subscriptions.
use crate::storage_support::{BoardTransaction, current_activity_sequence, storage_error};
use crate::thread_subscription_row_decoding::{
    StoredSubscriptionWindowRow, StoredThreadSubscriptionRow, corrupt_subscription,
    decode_thread_subscription_record,
};
use message_board::*;

pub(super) async fn load_subscription_row(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
) -> Result<Option<StoredThreadSubscriptionRow>, BoardError> {
    let (scope_kind, scope_id) = scope.kind_and_id();
    sqlx::query_as!(
        StoredThreadSubscriptionRow,
        "SELECT subscription.reader_key,identity.kind AS reader_kind,identity.service_id, \
           identity.endpoint_id,identity.session_id,identity.human_id, \
           subscription.scope_kind,subscription.scope_id,subscription.mode, \
           subscription.when_idle,subscription.quiet_seconds,subscription.cap_seconds, \
           subscription.lifetime_seconds,subscription.renewed_at,subscription.expires_at, \
           subscription.state,subscription.end_reason,subscription.ended_at, \
           subscription.last_outcome,subscription.generation \
         FROM thread_subscriptions subscription \
         JOIN board_identities identity ON identity.identity_key=subscription.reader_key \
         WHERE subscription.reader_key=? AND subscription.scope_kind=? AND subscription.scope_id=?",
        reader_key,
        scope_kind,
        scope_id,
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)
}

async fn load_subscription_windows(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
) -> Result<Vec<StoredSubscriptionWindowRow>, BoardError> {
    let rows = match scope {
        SubscriptionScope::Thread { root_message_id } => {
            sqlx::query_as!(
                StoredSubscriptionWindowRow,
                "SELECT root_id,opened_at,last_arrival_at,0 AS pending_count, \
                   held_since,retry_not_before,retry_attempts,window_id \
                 FROM subscription_windows WHERE reader_key=? AND root_id=?",
                reader_key,
                root_message_id.as_str(),
            )
            .fetch_all(&mut **transaction)
            .await
            .map_err(storage_error)?
        }
        SubscriptionScope::Topic { topic_id } => {
            sqlx::query_as!(
                StoredSubscriptionWindowRow,
                "SELECT window.root_id,window.opened_at,window.last_arrival_at,0 AS pending_count, \
                   window.held_since,window.retry_not_before, \
                   window.retry_attempts,window.window_id \
                 FROM subscription_windows window \
                 JOIN board_messages root ON root.message_id=window.root_id AND root.root_id IS NULL \
                 WHERE window.reader_key=? AND root.topic_id=? \
                   AND NOT EXISTS(SELECT 1 FROM thread_subscriptions thread_override \
                     WHERE thread_override.reader_key=window.reader_key \
                       AND thread_override.scope_kind='thread' AND thread_override.scope_id=window.root_id) \
                 ORDER BY window.root_id",
                reader_key,
                topic_id.as_str(),
            )
            .fetch_all(&mut **transaction)
            .await
            .map_err(storage_error)?
        }
    };
    let latest = current_activity_sequence(transaction).await?;
    let mut records = Vec::with_capacity(rows.len());
    for mut row in rows {
        let root_message_id =
            MessageId::try_from(row.root_id.clone()).map_err(|_| corrupt_subscription("rootId"))?;
        let pending_count = crate::subscription_window_records::pending_message_count(
            transaction,
            reader_key,
            &root_message_id,
            latest,
        )
        .await?;
        row.pending_count = pending_count;
        records.push(row);
    }
    Ok(records)
}

pub(super) async fn load_subscription_record(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
) -> Result<Option<ThreadSubscriptionRecord>, BoardError> {
    let Some(row) = load_subscription_row(transaction, reader_key, scope).await? else {
        return Ok(None);
    };
    let windows = load_subscription_windows(transaction, reader_key, scope).await?;
    let record = decode_thread_subscription_record(row, windows)?;
    Ok(Some(record))
}
pub(super) fn decode_subscription_scope(
    scope_kind: &str,
    scope_id: String,
) -> Result<SubscriptionScope, BoardError> {
    match scope_kind {
        "thread" => MessageId::try_from(scope_id)
            .map(SubscriptionScope::thread)
            .map_err(|_| corrupt_subscription("scopeId")),
        "topic" => TopicId::try_from(scope_id)
            .map(SubscriptionScope::topic)
            .map_err(|_| corrupt_subscription("scopeId")),
        _ => Err(corrupt_subscription("scopeKind")),
    }
}

pub(super) fn subscription_resource(scope: &SubscriptionScope) -> ResourceIdentity {
    match scope {
        SubscriptionScope::Thread { root_message_id } => ResourceIdentity::Thread {
            root_message_id: root_message_id.clone(),
        },
        SubscriptionScope::Topic { topic_id } => ResourceIdentity::Topic {
            topic_id: topic_id.clone(),
        },
    }
}
