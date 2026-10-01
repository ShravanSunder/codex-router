//! Atomically settle selected subscription batches and record retry or hold state.
use crate::BoardStore;
use crate::message_records::activity_sequence;
use crate::participant_records::advance_participant_last_seen;
use crate::storage_support::{
    BoardTransaction, current_activity_sequence, identity_key, invalid_record, storage_error,
};
use crate::subscription_window_records::load_window;
use crate::thread_batch_selection::pending_message_count;
use crate::thread_delivery_position_writer::write_delivered_position_if_valid;
use crate::thread_subscription_lifecycle_records::get_covering_subscription;
use crate::thread_subscription_row_decoding::encode_utc_timestamp;
use chrono::{DateTime, Utc};
use message_board::*;
use sqlx::Connection;

fn stored_subscription_generation(generation: SubscriptionGeneration) -> Result<i64, BoardError> {
    i64::try_from(generation.get()).map_err(|_| invalid_record())
}
impl BoardStore {
    /// Release an unhanded selection without changing its window or delivery position.
    /// A newer selection, replacement window, or already-settled fence is left alone.
    pub async fn release_subscription_batch(
        &mut self,
        reader: &Identity,
        settlement: &SubscriptionBatchSettlement,
    ) -> Result<(), BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        for root in &settlement.roots {
            let through =
                i64::try_from(root.delivered_through.get()).map_err(|_| invalid_record())?;
            sqlx::query!(
                "UPDATE subscription_windows SET in_flight_through=NULL,residual_opened_at=NULL \
                 WHERE reader_key=? AND root_id=? AND window_id=? AND in_flight_through=?",
                reader_key,
                root.root_message_id.as_str(),
                root.window_id.as_str(),
                through,
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        transaction.commit().await.map_err(storage_error)
    }

    /// Settle accepted, outcome-unknown, or deliberately dropped activity atomically.
    pub async fn settle_subscription_batch(
        &mut self,
        reader: &Identity,
        settlement: &SubscriptionBatchSettlement,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        if matches!(
            outcome,
            SubscriptionDeliveryOutcome::Rejected { .. }
                | SubscriptionDeliveryOutcome::NotSubmitted { .. }
        ) {
            return Err(BoardError::invalid_field(
                "outcome",
                "rejected and not-submitted outcomes must use a retry or hold mark",
            ));
        }
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        for root in &settlement.roots {
            settle_root_in_transaction(&mut transaction, &reader_key, root, &outcome).await?;
        }
        transaction.commit().await.map_err(storage_error)
    }

    /// Mark a selected Batch held without advancing Delivered.
    pub async fn mark_subscription_batch_held(
        &mut self,
        reader: &Identity,
        settlement: &SubscriptionBatchSettlement,
        now: DateTime<Utc>,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        self.mark_subscription_batch_pending(reader, settlement, now, None, outcome)
            .await
    }

    /// Record a rejected attempt and its retry deadline without advancing Delivered.
    pub async fn mark_subscription_batch_retry(
        &mut self,
        reader: &Identity,
        settlement: &SubscriptionBatchSettlement,
        now: DateTime<Utc>,
        retry_not_before: DateTime<Utc>,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        self.mark_subscription_batch_pending(
            reader,
            settlement,
            now,
            Some(retry_not_before),
            outcome,
        )
        .await
    }

    async fn mark_subscription_batch_pending(
        &mut self,
        reader: &Identity,
        settlement: &SubscriptionBatchSettlement,
        now: DateTime<Utc>,
        retry_not_before: Option<DateTime<Utc>>,
        outcome: SubscriptionDeliveryOutcome,
    ) -> Result<(), BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let latest = current_activity_sequence(&mut transaction).await?;
        let outcome_json =
            serde_json::to_string(&outcome).map_err(|_| BoardError::board_unavailable())?;
        for root in &settlement.roots {
            let pending_count =
                pending_message_count(&mut transaction, &reader_key, &root.root_message_id, latest)
                    .await?;
            let window_write = if pending_count <= 0 {
                sqlx::query!(
                    "DELETE FROM subscription_windows WHERE reader_key=? AND root_id=? AND window_id=?",
                    reader_key,
                    root.root_message_id.as_str(),
                    root.window_id.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?
            } else {
                sqlx::query!(
                    "UPDATE subscription_windows SET \
                       held_since=CASE WHEN ? IS NULL THEN COALESCE(held_since,?) ELSE NULL END, \
                       retry_not_before=?,retry_attempts=CASE WHEN ? IS NULL THEN retry_attempts ELSE retry_attempts+1 END, \
                       in_flight_through=NULL,residual_opened_at=NULL \
                     WHERE reader_key=? AND root_id=? AND window_id=?",
                    retry_not_before.map(encode_utc_timestamp),
                    encode_utc_timestamp(now),
                    retry_not_before.map(encode_utc_timestamp),
                    retry_not_before.map(encode_utc_timestamp),
                    reader_key,
                    root.root_message_id.as_str(),
                    root.window_id.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?
            };
            if window_write.rows_affected() > 0
                && subscription_generation_matches(
                    &mut transaction,
                    &reader_key,
                    &root.subscription_scope,
                    root.subscription_generation,
                )
                .await?
            {
                set_subscription_outcome(
                    &mut transaction,
                    &reader_key,
                    &root.subscription_scope,
                    root.subscription_generation,
                    outcome_json.as_str(),
                )
                .await?;
            }
        }
        transaction.commit().await.map_err(storage_error)
    }

    /// Skip all currently pending activity for the selected roots (drop policy).
    pub async fn drop_subscription_roots(
        &mut self,
        reader: &Identity,
        roots: &[MessageId],
        now: DateTime<Utc>,
    ) -> Result<(), BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        for root_message_id in roots {
            let Some(subscription) =
                get_covering_subscription(&mut transaction, &reader_key, root_message_id, now)
                    .await?
            else {
                continue;
            };
            if subscription.mode != SubscriptionMode::Deliver
                || subscription.when_idle != WhenIdle::Drop
                || !subscription.state.is_delivery_eligible()
            {
                continue;
            }
            let Some(window) = load_window(&mut transaction, &reader_key, root_message_id).await?
            else {
                continue;
            };
            let pending_through: Option<i64> = sqlx::query_scalar!(
                "SELECT MAX(activity.activity_sequence) FROM board_activity activity \
                 LEFT JOIN thread_delivery_positions position \
                   ON position.reader_key=? AND position.root_id=? \
                 JOIN thread_watches watch ON watch.reader_key=? AND watch.root_id=? AND watch.active=1 \
                 WHERE ((activity.root_id=? AND activity.kind='threadMessageCreated') \
                   OR (activity.message_id=? AND activity.kind='mainMessageCreated')) \
                   AND activity.actor_key<>? \
                   AND activity.activity_sequence>COALESCE(position.delivered_through,watch.starts_after_activity)",
                reader_key,
                root_message_id.as_str(),
                reader_key,
                root_message_id.as_str(),
                root_message_id.as_str(),
                root_message_id.as_str(),
                reader_key,
            )
            .fetch_one(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let Some(pending_through) = pending_through else {
                sqlx::query!(
                    "DELETE FROM subscription_windows WHERE reader_key=? AND root_id=? AND window_id=?",
                    reader_key,
                    root_message_id.as_str(),
                    window.window_id.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                continue;
            };
            let settlement_root = SubscriptionBatchRootSettlement {
                root_message_id: root_message_id.clone(),
                window_id: window.window_id,
                delivered_through: activity_sequence(pending_through)?,
                subscription_scope: subscription.scope,
                subscription_generation: subscription.generation,
            };
            settle_root_in_transaction(
                &mut transaction,
                &reader_key,
                &settlement_root,
                &SubscriptionDeliveryOutcome::Dropped,
            )
            .await?;
        }
        transaction.commit().await.map_err(storage_error)
    }
}

async fn subscription_generation_matches(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
    generation: SubscriptionGeneration,
) -> Result<bool, BoardError> {
    let (scope_kind, scope_id) = scope.kind_and_id();
    let current = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM thread_subscriptions WHERE reader_key=? \
         AND scope_kind=? AND scope_id=? AND generation=? AND state IN ('active','draining'))",
        reader_key,
        scope_kind,
        scope_id,
        stored_subscription_generation(generation)?,
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(current != 0)
}

async fn set_subscription_outcome(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
    generation: SubscriptionGeneration,
    outcome_json: &str,
) -> Result<(), BoardError> {
    let (scope_kind, scope_id) = scope.kind_and_id();
    sqlx::query!(
        "UPDATE thread_subscriptions SET last_outcome=? \
         WHERE reader_key=? AND scope_kind=? AND scope_id=? AND generation=? \
           AND state IN ('active','draining')",
        outcome_json,
        reader_key,
        scope_kind,
        scope_id,
        stored_subscription_generation(generation)?,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn settle_root_in_transaction(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root: &SubscriptionBatchRootSettlement,
    outcome: &SubscriptionDeliveryOutcome,
) -> Result<(), BoardError> {
    let latest = current_activity_sequence(transaction).await?;
    let delivered_position_advanced = write_delivered_position_if_valid(
        transaction,
        reader_key,
        &root.root_message_id,
        root.delivered_through,
    )
    .await?;
    if delivered_position_advanced {
        let delivered_value =
            i64::try_from(root.delivered_through.get()).map_err(|_| invalid_record())?;
        advance_participant_last_seen(
            transaction,
            reader_key,
            &root.root_message_id,
            delivered_value,
        )
        .await?;
    }
    let pending_count =
        pending_message_count(transaction, reader_key, &root.root_message_id, latest).await?;
    let window_write = if pending_count == 0 {
        sqlx::query!(
            "DELETE FROM subscription_windows WHERE reader_key=? AND root_id=? AND window_id=?",
            reader_key,
            root.root_message_id.as_str(),
            root.window_id.as_str(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?
    } else {
        sqlx::query!(
            "UPDATE subscription_windows SET \
               opened_at=COALESCE(residual_opened_at,opened_at),held_since=NULL, \
               retry_not_before=NULL,retry_attempts=0,in_flight_through=NULL,residual_opened_at=NULL \
             WHERE reader_key=? AND root_id=? AND window_id=?",
            reader_key,
            root.root_message_id.as_str(),
            root.window_id.as_str(),
        )
        .execute(&mut **transaction)
        .await
        .map_err(storage_error)?
    };
    if window_write.rows_affected() > 0 {
        let outcome_json =
            serde_json::to_string(outcome).map_err(|_| BoardError::board_unavailable())?;
        if subscription_generation_matches(
            transaction,
            reader_key,
            &root.subscription_scope,
            root.subscription_generation,
        )
        .await?
        {
            set_subscription_outcome(
                transaction,
                reader_key,
                &root.subscription_scope,
                root.subscription_generation,
                outcome_json.as_str(),
            )
            .await?;
        }
    }
    Ok(())
}
