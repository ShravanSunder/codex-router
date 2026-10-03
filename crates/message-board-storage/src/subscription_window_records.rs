//! Persist per-root batching windows, due selection, and post hooks.
use crate::BoardStore;
use crate::storage_support::{
    BoardTransaction, current_activity_sequence, identity_key, invalid_record, storage_error,
    validate_topic_watch_boundary,
};
#[path = "subscription_window_records/root_activity_selection.rs"]
mod root_activity_selection;
use crate::thread_subscription_lifecycle_records::{
    CoveringSubscription, ensure_topic_root_watch, get_covering_subscription,
    renew_covering_subscription,
};
use crate::thread_subscription_row_decoding::{
    corrupt_subscription, decode_utc_timestamp, encode_utc_timestamp,
};
use chrono::{DateTime, Duration, Utc};
use message_board::*;
pub(crate) use root_activity_selection::pending_message_count;
use root_activity_selection::{SubscriptionRootSelection, select_pending_root_notices};
use sqlx::Connection;
use std::collections::HashMap;

pub(super) struct StoredSelectionWindow {
    pub(super) window_id: SubscriptionWindowId,
    pub(super) opened_at: DateTime<Utc>,
    pub(super) last_arrival_at: DateTime<Utc>,
    pub(super) held_since: Option<DateTime<Utc>>,
    pub(super) retry_not_before: Option<DateTime<Utc>>,
    pub(super) in_flight_through: Option<i64>,
}

struct PostCoverageRow {
    reader_key: String,
    mode: String,
    state: String,
}
fn policy_deadline(
    opened_at: DateTime<Utc>,
    last_arrival_at: DateTime<Utc>,
    quiet_seconds: u64,
    cap_seconds: u64,
) -> Result<DateTime<Utc>, BoardError> {
    let quiet = i64::try_from(quiet_seconds).map_err(|_| invalid_record())?;
    let cap = i64::try_from(cap_seconds).map_err(|_| invalid_record())?;
    let quiet_deadline = last_arrival_at
        .checked_add_signed(Duration::seconds(quiet))
        .ok_or_else(invalid_record)?;
    let cap_deadline = opened_at
        .checked_add_signed(Duration::seconds(cap))
        .ok_or_else(invalid_record)?;
    Ok(quiet_deadline.min(cap_deadline))
}

pub(super) async fn load_window(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
) -> Result<Option<StoredSelectionWindow>, BoardError> {
    let row = sqlx::query!(
        "SELECT window_id,opened_at,last_arrival_at,held_since,retry_not_before, \
           retry_attempts,in_flight_through \
         FROM subscription_windows WHERE reader_key=? AND root_id=?",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    row.map(|row| {
        let window_id = SubscriptionWindowId::try_from(row.window_id)
            .map_err(|_| corrupt_subscription("windowId"))?;
        let _retry_attempts =
            u32::try_from(row.retry_attempts).map_err(|_| corrupt_subscription("retryAttempts"))?;
        Ok(StoredSelectionWindow {
            window_id,
            opened_at: decode_utc_timestamp("openedAt", row.opened_at.as_str())?,
            last_arrival_at: decode_utc_timestamp("lastArrivalAt", row.last_arrival_at.as_str())?,
            held_since: row
                .held_since
                .map(|value| decode_utc_timestamp("heldSince", value.as_str()))
                .transpose()?,
            retry_not_before: row
                .retry_not_before
                .map(|value| decode_utc_timestamp("retryNotBefore", value.as_str()))
                .transpose()?,
            in_flight_through: row.in_flight_through,
        })
    })
    .transpose()
}

fn root_is_due(
    window: &StoredSelectionWindow,
    policy: &CoveringSubscription,
    now: DateTime<Utc>,
) -> Result<bool, BoardError> {
    if window
        .retry_not_before
        .is_some_and(|retry_not_before| retry_not_before > now)
    {
        return Ok(false);
    }
    Ok(window
        .retry_not_before
        .is_some_and(|retry_not_before| retry_not_before <= now)
        || policy_deadline(
            window.opened_at,
            window.last_arrival_at,
            policy.quiet_seconds,
            policy.cap_seconds,
        )? <= now)
}

pub(crate) async fn rescan_missing_windows_in_transaction(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    now: DateTime<Utc>,
) -> Result<u64, BoardError> {
    let now_text = encode_utc_timestamp(now);
    let scopes = sqlx::query!(
        "SELECT scope_kind,scope_id FROM thread_subscriptions \
         WHERE reader_key=? AND state IN ('active','draining') AND expires_at>? AND mode<>'off' \
         ORDER BY scope_kind,scope_id",
        reader_key,
        now_text,
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let mut opened_count = 0_u64;
    let latest = current_activity_sequence(transaction).await?;
    let existing_roots = sqlx::query_scalar!(
        "SELECT root_id FROM subscription_windows WHERE reader_key=? ORDER BY root_id",
        reader_key,
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    for root_id in existing_roots {
        let root_message_id = MessageId::try_from(root_id).map_err(|_| invalid_record())?;
        if pending_message_count(transaction, reader_key, &root_message_id, latest).await? <= 0 {
            sqlx::query!(
                "DELETE FROM subscription_windows WHERE reader_key=? AND root_id=?",
                reader_key,
                root_message_id.as_str(),
            )
            .execute(&mut **transaction)
            .await
            .map_err(storage_error)?;
        }
    }
    for scope_row in scopes {
        let roots = match scope_row.scope_kind.as_str() {
            "thread" => vec![
                MessageId::try_from(scope_row.scope_id)
                    .map_err(|_| corrupt_subscription("scopeId"))?,
            ],
            "topic" => {
                let topic_id = TopicId::try_from(scope_row.scope_id)
                    .map_err(|_| corrupt_subscription("scopeId"))?;
                let topic_watch_start: i64 = sqlx::query_scalar!(
                    "SELECT starts_after_activity FROM topic_watches \
                     WHERE reader_key=? AND topic_id=? AND active=1",
                    reader_key,
                    topic_id.as_str(),
                )
                .fetch_optional(&mut **transaction)
                .await
                .map_err(storage_error)?
                .ok_or_else(|| corrupt_subscription("topicWatch"))?;
                validate_topic_watch_boundary(topic_watch_start, latest, &topic_id)?;
                let topic =
                    crate::board_topic_records::require_topic(transaction, &topic_id).await?;
                let board =
                    crate::board_topic_records::require_board(transaction, &topic.board_id).await?;
                let root_ids = sqlx::query_scalar!(
                    "SELECT root.message_id FROM board_messages root \
                     WHERE root.topic_id=? AND root.root_id IS NULL \
                       AND NOT EXISTS(SELECT 1 FROM thread_subscriptions thread_override \
                         WHERE thread_override.reader_key=? AND thread_override.scope_kind='thread' \
                           AND thread_override.scope_id=root.message_id) \
                     ORDER BY root.message_id",
                    topic_id.as_str(),
                    reader_key,
                )
                .fetch_all(&mut **transaction)
                .await
                .map_err(storage_error)?;
                for root_id in &root_ids {
                    let root_id = MessageId::try_from(root_id.clone())
                        .map_err(|_| corrupt_subscription("scopeId"))?;
                    ensure_topic_root_watch(
                        transaction,
                        reader_key,
                        &board.project_id,
                        &topic_id,
                        &root_id,
                        topic_watch_start,
                    )
                    .await?;
                }
                root_ids
                    .into_iter()
                    .map(|root_id| {
                        MessageId::try_from(root_id).map_err(|_| corrupt_subscription("scopeId"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
            _ => return Err(corrupt_subscription("scopeKind")),
        };
        for root_message_id in roots {
            let pending_count =
                pending_message_count(transaction, reader_key, &root_message_id, latest).await?;
            if pending_count <= 0 {
                continue;
            }
            let exists = sqlx::query_scalar!(
                "SELECT EXISTS(SELECT 1 FROM subscription_windows WHERE reader_key=? AND root_id=?)",
                reader_key,
                root_message_id.as_str(),
            )
            .fetch_one(&mut **transaction)
            .await
            .map_err(storage_error)?;
            if exists != 0 {
                continue;
            }
            insert_window(transaction, reader_key, &root_message_id, now).await?;
            opened_count = opened_count.checked_add(1).ok_or_else(invalid_record)?;
        }
    }
    Ok(opened_count)
}

async fn insert_window(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    let now_text = encode_utc_timestamp(now);
    let window_id = SubscriptionWindowId::generate();
    sqlx::query!(
        "INSERT INTO subscription_windows(reader_key,root_id,opened_at,last_arrival_at, \
           held_since,retry_not_before,retry_attempts,window_id,in_flight_through,residual_opened_at) \
         VALUES(?,?,?,?,NULL,NULL,0,?,NULL,NULL) ON CONFLICT(reader_key,root_id) DO NOTHING",
        reader_key,
        root_message_id.as_str(),
        now_text,
        now_text,
        window_id.as_str(),
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}
impl BoardStore {
    /// Return due roots oldest-window-first so a capped notice does not starve older windows.
    pub async fn due_subscription_roots(
        &mut self,
        reader: &Identity,
        now: DateTime<Utc>,
    ) -> Result<Vec<MessageId>, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let latest = current_activity_sequence(&mut transaction).await?;
        let roots = sqlx::query_scalar!(
            "SELECT root_id FROM subscription_windows WHERE reader_key=? ORDER BY opened_at,root_id",
            reader_key,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut due_roots = Vec::new();
        for root_id in roots {
            let root_message_id =
                MessageId::try_from(root_id).map_err(|_| corrupt_subscription("rootId"))?;
            let Some(subscription) =
                get_covering_subscription(&mut transaction, &reader_key, &root_message_id, now)
                    .await?
            else {
                continue;
            };
            if subscription.mode == SubscriptionMode::Off
                || !subscription.state.is_delivery_eligible()
            {
                continue;
            }
            if pending_message_count(&mut transaction, &reader_key, &root_message_id, latest)
                .await?
                <= 0
            {
                continue;
            }
            let Some(window) = load_window(&mut transaction, &reader_key, &root_message_id).await?
            else {
                continue;
            };
            if window.in_flight_through.is_none() && root_is_due(&window, &subscription, now)? {
                due_roots.push(root_message_id);
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(due_roots)
    }

    /// Mark the largest fitting prefix of up to 20 roots in flight and return bodyless locators.
    ///
    /// Roots omitted by either cap remain unchanged and due for the next selection cycle.
    pub async fn select_subscription_notice(
        &mut self,
        reader: &Identity,
        roots: &[MessageId],
        now: DateTime<Utc>,
        maximum_root_notice_bytes: usize,
    ) -> Result<(SubscriptionBatch, SubscriptionBatchSettlement), BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let already_in_flight = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM subscription_windows \
             WHERE reader_key=? AND in_flight_through IS NOT NULL)",
            reader_key,
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if already_in_flight != 0 {
            return Err(BoardError::invalid_field(
                "subscriptionNotice",
                "this Reader already has a notice in flight",
            ));
        }
        let latest = current_activity_sequence(&mut transaction).await?;
        let mut selection_roots = Vec::new();
        let mut window_facts = HashMap::new();
        for root_message_id in roots {
            let Some(subscription) =
                get_covering_subscription(&mut transaction, &reader_key, root_message_id, now)
                    .await?
            else {
                continue;
            };
            if subscription.mode == SubscriptionMode::Off
                || !subscription.state.is_delivery_eligible()
            {
                continue;
            }
            let Some(window) = load_window(&mut transaction, &reader_key, root_message_id).await?
            else {
                continue;
            };
            if window.in_flight_through.is_some() {
                return Err(BoardError::invalid_field(
                    "subscriptionNotice",
                    "a selected root is already in flight",
                ));
            }
            selection_roots.push(SubscriptionRootSelection {
                root_message_id: root_message_id.clone(),
            });
            window_facts.insert(
                root_message_id.clone(),
                (window.window_id, window.held_since, subscription),
            );
        }
        let mut root_notices =
            select_pending_root_notices(&mut transaction, &reader_key, &selection_roots, latest)
                .await?;
        if serde_json::to_vec(&Vec::<PendingRootNotice>::new())
            .map_err(|_| BoardError::board_unavailable())?
            .len()
            > maximum_root_notice_bytes
        {
            return Err(BoardError::board_unavailable());
        }
        let mut selected_root_count = root_notices.len();
        while selected_root_count > 0 {
            let selected_prefix = root_notices
                .iter()
                .take(selected_root_count)
                .collect::<Vec<_>>();
            let selected_prefix_bytes = serde_json::to_vec(&selected_prefix)
                .map_err(|_| BoardError::board_unavailable())?
                .len();
            if selected_prefix_bytes <= maximum_root_notice_bytes {
                break;
            }
            selected_root_count -= 1;
        }
        if !root_notices.is_empty() && selected_root_count == 0 {
            return Err(BoardError::board_unavailable());
        }
        root_notices.truncate(selected_root_count);
        let mut batch_held_since: Option<DateTime<Utc>> = None;
        let mut batch_draining = false;
        let mut settlements = Vec::with_capacity(root_notices.len());
        for selected in &root_notices {
            let Some((window_id, held_since, subscription)) = window_facts.get(&selected.root_id)
            else {
                return Err(invalid_record());
            };
            if let Some(held_since) = held_since {
                batch_held_since = Some(
                    batch_held_since.map_or(*held_since, |existing| existing.min(*held_since)),
                );
            }
            batch_draining |= matches!(subscription.state, SubscriptionState::Draining);
            let through =
                i64::try_from(selected.through_sequence.get()).map_err(|_| invalid_record())?;
            sqlx::query!(
                "UPDATE subscription_windows SET in_flight_through=?,residual_opened_at=NULL \
                 WHERE reader_key=? AND root_id=? AND window_id=?",
                through,
                reader_key,
                selected.root_id.as_str(),
                window_id.as_str(),
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            settlements.push(SubscriptionBatchRootSettlement {
                root_message_id: selected.root_id.clone(),
                window_id: window_id.clone(),
                delivered_through: selected.through_sequence,
                subscription_scope: subscription.scope.clone(),
                subscription_generation: subscription.generation,
            });
        }
        let held_since = batch_held_since;
        let batch = SubscriptionBatch {
            batch_id: BatchId::generate(),
            held: held_since.is_some(),
            held_since,
            draining: batch_draining,
            roots: root_notices,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok((batch, SubscriptionBatchSettlement { roots: settlements }))
    }
}

pub(crate) async fn record_subscription_post(
    transaction: &mut BoardTransaction<'_>,
    author_key: &str,
    topic_id: &TopicId,
    root_message_id: &MessageId,
    activity_sequence: i64,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    let before_post = activity_sequence
        .checked_sub(1)
        .ok_or_else(invalid_record)?;
    let topic_subscribers = sqlx::query_scalar!(
        "SELECT reader_key FROM thread_subscriptions WHERE scope_kind='topic' AND scope_id=? \
         AND state='active' AND expires_at>? ORDER BY reader_key",
        topic_id.as_str(),
        encode_utc_timestamp(now),
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let topic = crate::board_topic_records::require_topic(transaction, topic_id).await?;
    let board = crate::board_topic_records::require_board(transaction, &topic.board_id).await?;
    for reader_key in topic_subscribers {
        ensure_topic_root_watch(
            transaction,
            reader_key.as_str(),
            &board.project_id,
            topic_id,
            root_message_id,
            before_post,
        )
        .await?;
    }

    renew_covering_subscription(transaction, author_key, root_message_id, now).await?;
    let candidates = sqlx::query_as!(
        PostCoverageRow,
        "SELECT subscription.reader_key,subscription.mode,subscription.state \
         FROM thread_subscriptions subscription \
         WHERE subscription.state IN ('active','draining') AND subscription.expires_at>? \
           AND ((subscription.scope_kind='thread' AND subscription.scope_id=?) \
             OR (subscription.scope_kind='topic' AND subscription.scope_id=? \
               AND NOT EXISTS(SELECT 1 FROM thread_subscriptions thread_override \
                 WHERE thread_override.reader_key=subscription.reader_key \
                   AND thread_override.scope_kind='thread' AND thread_override.scope_id=?))) \
         ORDER BY subscription.reader_key",
        encode_utc_timestamp(now),
        root_message_id.as_str(),
        topic_id.as_str(),
        root_message_id.as_str(),
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    for candidate in candidates {
        if candidate.reader_key == author_key {
            continue;
        }
        if candidate.mode != "deliver" && candidate.mode != "poll" && candidate.mode != "off" {
            return Err(corrupt_subscription("mode"));
        }
        if candidate.state != "active" && candidate.state != "draining" {
            return Err(corrupt_subscription("state"));
        }
        if candidate.mode == "off" {
            continue;
        }
        let window_id = SubscriptionWindowId::generate();
        let now_text = encode_utc_timestamp(now);
        sqlx::query!(
            "INSERT INTO subscription_windows(reader_key,root_id,opened_at,last_arrival_at, \
               held_since,retry_not_before,retry_attempts,window_id,in_flight_through,residual_opened_at) \
             VALUES(?,?,?,?,NULL,NULL,0,?,NULL,NULL) \
             ON CONFLICT(reader_key,root_id) DO UPDATE SET \
               last_arrival_at=excluded.last_arrival_at, \
               residual_opened_at=CASE WHEN subscription_windows.in_flight_through IS NOT NULL \
                 AND subscription_windows.residual_opened_at IS NULL \
                 THEN excluded.last_arrival_at ELSE subscription_windows.residual_opened_at END",
            candidate.reader_key,
            root_message_id.as_str(),
            now_text,
            now_text,
            window_id.as_str(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?;
    }
    Ok(())
}
