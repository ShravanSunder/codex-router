//! Public subscription storage operations and their row-write helpers.
use crate::BoardStore;
use crate::message_records::require_thread;
use crate::participant_row_decoding::require_open_participant;
use crate::storage_support::{
    BoardTransaction, current_activity_sequence, ensure_identity, identity_key, invalid_record,
    storage_error,
};
use crate::subscription_window_records::pending_message_count;
use crate::thread_subscription_lifecycle_records::{
    activate_scope_watch, end_expired_rows, end_subscription,
};
use crate::thread_subscription_row_decoding::encode_utc_timestamp;
use crate::thread_subscription_row_reads::{
    decode_subscription_scope, load_subscription_record, subscription_resource,
};
use chrono::{DateTime, Duration, Utc};
use message_board::*;
use sqlx::Connection;

pub(super) fn encode_scope(scope: &SubscriptionScope) -> (&'static str, &str) {
    scope.kind_and_id()
}

pub(super) fn policy_error(error: InvalidSubscriptionField) -> BoardError {
    BoardError::invalid_field(error.field, error.requirement)
}

fn encode_outcome(
    outcome: Option<&SubscriptionDeliveryOutcome>,
) -> Result<Option<String>, BoardError> {
    outcome
        .map(|value| serde_json::to_string(value).map_err(|_| BoardError::board_unavailable()))
        .transpose()
}

pub(super) fn expiry_at(
    now: DateTime<Utc>,
    lifetime: SubscriptionLifetime,
) -> Result<DateTime<Utc>, BoardError> {
    let seconds = i64::try_from(lifetime.seconds()).map_err(|_| invalid_record())?;
    now.checked_add_signed(Duration::seconds(seconds))
        .ok_or_else(invalid_record)
}

pub(super) fn stored_generation(generation: SubscriptionGeneration) -> Result<i64, BoardError> {
    i64::try_from(generation.get()).map_err(|_| invalid_record())
}

pub(super) struct SubscriptionWrite<'a> {
    pub reader_key: &'a str,
    pub scope: &'a SubscriptionScope,
    pub policy: &'a SubscriptionPolicy,
    pub state: SubscriptionState,
    pub now: DateTime<Utc>,
    pub generation: SubscriptionGeneration,
    pub last_outcome: Option<&'a SubscriptionDeliveryOutcome>,
}

pub(super) async fn upsert_subscription(
    transaction: &mut BoardTransaction<'_>,
    write: SubscriptionWrite<'_>,
) -> Result<(), BoardError> {
    let (scope_kind, scope_id) = encode_scope(write.scope);
    let end_reason = write.state.end_reason().map(EndReason::as_str);
    let ended_at = if end_reason.is_some() {
        Some(encode_utc_timestamp(write.now))
    } else {
        None
    };
    let expires_at = expiry_at(write.now, write.policy.lifetime())?;
    let last_outcome = encode_outcome(write.last_outcome)?;
    sqlx::query!(
        "INSERT INTO thread_subscriptions(reader_key,scope_kind,scope_id,mode,when_idle, \
           quiet_seconds,cap_seconds,lifetime_seconds,renewed_at,expires_at,state,end_reason, \
           ended_at,last_outcome,generation) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
         ON CONFLICT(reader_key,scope_kind,scope_id) DO UPDATE SET mode=excluded.mode, \
           when_idle=excluded.when_idle,quiet_seconds=excluded.quiet_seconds, \
           cap_seconds=excluded.cap_seconds,lifetime_seconds=excluded.lifetime_seconds, \
           renewed_at=excluded.renewed_at,expires_at=excluded.expires_at,state=excluded.state, \
           end_reason=excluded.end_reason,ended_at=excluded.ended_at, \
           last_outcome=excluded.last_outcome,generation=excluded.generation",
        write.reader_key,
        scope_kind,
        scope_id,
        write.policy.mode().as_str(),
        write.policy.when_idle().as_str(),
        i64::try_from(write.policy.timing().quiet_seconds()).map_err(|_| invalid_record())?,
        i64::try_from(write.policy.timing().cap_seconds()).map_err(|_| invalid_record())?,
        i64::try_from(write.policy.lifetime().seconds()).map_err(|_| invalid_record())?,
        encode_utc_timestamp(write.now),
        encode_utc_timestamp(expires_at),
        encode_state(write.state),
        end_reason,
        ended_at,
        last_outcome,
        stored_generation(write.generation)?,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

fn encode_state(state: SubscriptionState) -> &'static str {
    match state {
        SubscriptionState::Active => "active",
        SubscriptionState::Draining => "draining",
        SubscriptionState::Ended { .. } => "ended",
    }
}

pub(super) fn encode_end_reason(reason: EndReason) -> &'static str {
    reason.as_str()
}

pub(super) async fn delete_scope_windows(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
) -> Result<(), BoardError> {
    match scope {
        SubscriptionScope::Thread { root_message_id } => {
            sqlx::query!(
                "DELETE FROM subscription_windows WHERE reader_key=? AND root_id=?",
                reader_key,
                root_message_id.as_str(),
            )
            .execute(&mut **transaction)
            .await
            .map_err(storage_error)?;
        }
        SubscriptionScope::Topic { topic_id } => {
            sqlx::query!(
                "DELETE FROM subscription_windows WHERE reader_key=? AND root_id IN ( \
                   SELECT root.message_id FROM board_messages root \
                   WHERE root.root_id IS NULL AND root.topic_id=? \
                     AND NOT EXISTS(SELECT 1 FROM thread_subscriptions thread_override \
                       WHERE thread_override.reader_key=? AND thread_override.scope_kind='thread' \
                         AND thread_override.scope_id=root.message_id))",
                reader_key,
                topic_id.as_str(),
                reader_key,
            )
            .execute(&mut **transaction)
            .await
            .map_err(storage_error)?;
        }
    }
    Ok(())
}
struct SaveSubscriptionProps<'a> {
    reader_key: &'a str,
    reader: &'a Identity,
    scope: &'a SubscriptionScope,
    policy_patch: &'a SubscriptionPolicyPatch,
    now: DateTime<Utc>,
    activate_watch: bool,
}

async fn save_subscription(
    transaction: &mut BoardTransaction<'_>,
    props: SaveSubscriptionProps<'_>,
) -> Result<ThreadSubscriptionRecord, BoardError> {
    let latest = current_activity_sequence(transaction).await?;
    let mut existing = load_subscription_record(transaction, props.reader_key, props.scope).await?;
    let (policy, state, generation, last_outcome) = match existing.as_ref() {
        Some(record) => {
            let policy = record
                .policy()
                .apply_patch(props.reader, props.policy_patch)
                .map_err(policy_error)?;
            let state_changed = !matches!(record.state(), SubscriptionState::Active);
            let policy_changed = policy != *record.policy();
            let generation = if state_changed || policy_changed {
                record.generation().next().map_err(policy_error)?
            } else {
                record.generation()
            };
            (
                policy,
                SubscriptionState::Active,
                generation,
                record.last_outcome().cloned(),
            )
        }
        None => {
            let policy = SubscriptionPolicy::defaults_for(props.reader)
                .apply_patch(props.reader, props.policy_patch)
                .map_err(policy_error)?;
            (
                policy,
                SubscriptionState::Active,
                SubscriptionGeneration::new(1).map_err(policy_error)?,
                None,
            )
        }
    };
    if props.activate_watch {
        activate_scope_watch(transaction, props.reader_key, props.scope, latest).await?;
    }
    upsert_subscription(
        transaction,
        SubscriptionWrite {
            reader_key: props.reader_key,
            scope: props.scope,
            policy: &policy,
            state,
            now: props.now,
            generation,
            last_outcome: last_outcome.as_ref(),
        },
    )
    .await?;
    existing = load_subscription_record(transaction, props.reader_key, props.scope).await?;
    existing.ok_or_else(invalid_record)
}
impl BoardStore {
    /// Create, patch, or reactivate one subscription and its scope Watch atomically.
    pub async fn subscribe_thread_subscription(
        &mut self,
        request: ThreadSubscriptionSubscribeRequest,
        now: DateTime<Utc>,
    ) -> Result<ThreadSubscriptionRecord, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, &request.reader).await?;
        let _ = end_expired_rows(&mut transaction, now, Some(&reader_key)).await?;
        if let SubscriptionScope::Thread { root_message_id } = &request.scope {
            require_thread(&mut transaction, root_message_id).await?;
            require_open_participant(&mut transaction, &request.reader, root_message_id).await?;
        }
        let record = save_subscription(
            &mut transaction,
            SaveSubscriptionProps {
                reader_key: &reader_key,
                reader: &request.reader,
                scope: &request.scope,
                policy_patch: &request.policy,
                now,
                activate_watch: true,
            },
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Cancel one subscription without changing its Watch.
    pub async fn unsubscribe_thread_subscription(
        &mut self,
        request: ThreadSubscriptionUnsubscribeRequest,
        now: DateTime<Utc>,
    ) -> Result<ThreadSubscriptionRecord, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, &request.reader).await?;
        let _ = end_expired_rows(&mut transaction, now, Some(&reader_key)).await?;
        let Some(record) =
            load_subscription_record(&mut transaction, &reader_key, &request.scope).await?
        else {
            return Err(BoardError::resource_not_found(subscription_resource(
                &request.scope,
            )));
        };
        if record.state().is_delivery_eligible() {
            end_subscription(
                &mut transaction,
                &reader_key,
                &request.scope,
                EndReason::Cancelled,
                now,
            )
            .await?;
        }
        let record = load_subscription_record(&mut transaction, &reader_key, &request.scope)
            .await?
            .ok_or_else(invalid_record)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// Complete R20 after a resolved Thread subscription has drained every pending window.
    pub async fn complete_thread_subscription_drain(
        &mut self,
        reader: &Identity,
        root_message_id: &MessageId,
        now: DateTime<Utc>,
    ) -> Result<ThreadSubscriptionRecord, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = ensure_identity(&mut transaction, reader).await?;
        let scope = SubscriptionScope::thread(root_message_id.clone());
        let record = load_subscription_record(&mut transaction, &reader_key, &scope)
            .await?
            .ok_or_else(|| BoardError::resource_not_found(subscription_resource(&scope)))?;
        if record.state() != SubscriptionState::Draining {
            return Err(BoardError::invalid_field(
                "state",
                "only a draining Thread subscription can be completed",
            ));
        }
        let latest = current_activity_sequence(&mut transaction).await?;
        let pending_count =
            pending_message_count(&mut transaction, &reader_key, root_message_id, latest).await?;
        let has_windows = sqlx::query_scalar!(
            "SELECT EXISTS(SELECT 1 FROM subscription_windows WHERE reader_key=? AND root_id=?)",
            reader_key,
            root_message_id.as_str(),
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if pending_count > 0 || has_windows != 0 {
            return Err(BoardError::invalid_field(
                "windows",
                "the draining Thread subscription still has pending activity or a window",
            ));
        }
        end_subscription(
            &mut transaction,
            &reader_key,
            &scope,
            EndReason::Resolved,
            now,
        )
        .await?;
        let completed = load_subscription_record(&mut transaction, &reader_key, &scope)
            .await?
            .ok_or_else(invalid_record)?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(completed)
    }

    /// Read one subscription record, including an ended record if present.
    pub async fn get_thread_subscription_record(
        &mut self,
        reader: &Identity,
        scope: &SubscriptionScope,
    ) -> Result<Option<ThreadSubscriptionRecord>, BoardError> {
        let mut transaction = self.connection.begin().await.map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let record = load_subscription_record(&mut transaction, &reader_key, scope).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(record)
    }

    /// List active or draining subscription records after applying due expiry transitions.
    pub async fn list_reader_subscriptions(
        &mut self,
        reader: &Identity,
        now: DateTime<Utc>,
    ) -> Result<Vec<ThreadSubscriptionRecord>, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let _ = end_expired_rows(&mut transaction, now, Some(&reader_key)).await?;
        let rows = sqlx::query!(
            "SELECT scope_kind,scope_id FROM thread_subscriptions \
             WHERE reader_key=? AND state IN ('active','draining') \
             ORDER BY scope_kind,scope_id",
            reader_key,
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            let scope = decode_subscription_scope(row.scope_kind.as_str(), row.scope_id)?;
            let record = load_subscription_record(&mut transaction, &reader_key, &scope)
                .await?
                .ok_or_else(invalid_record)?;
            records.push(record);
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(records)
    }

    /// Renew one still-live subscription from its Reader's explicit action.
    pub async fn renew_thread_subscription(
        &mut self,
        reader: &Identity,
        scope: &SubscriptionScope,
        now: DateTime<Utc>,
    ) -> Result<Option<ThreadSubscriptionRecord>, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let reader_key = identity_key(reader);
        let _ = end_expired_rows(&mut transaction, now, Some(&reader_key)).await?;
        let Some(record) = load_subscription_record(&mut transaction, &reader_key, scope).await?
        else {
            transaction.commit().await.map_err(storage_error)?;
            return Ok(None);
        };
        if record.state().is_delivery_eligible() {
            let expires_at = expiry_at(now, record.policy().lifetime())?;
            let (scope_kind, scope_id) = encode_scope(scope);
            sqlx::query!(
                "UPDATE thread_subscriptions SET renewed_at=?,expires_at=? \
                 WHERE reader_key=? AND scope_kind=? AND scope_id=? AND generation=? \
                   AND state IN ('active','draining')",
                encode_utc_timestamp(now),
                encode_utc_timestamp(expires_at),
                reader_key,
                scope_kind,
                scope_id,
                stored_generation(record.generation())?,
            )
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        let renewed = load_subscription_record(&mut transaction, &reader_key, scope).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(renewed)
    }

    /// Open windows for covered roots with pending messages but no durable window.
    pub async fn rescan_missing_subscription_windows(
        &mut self,
        reader: &Identity,
        now: DateTime<Utc>,
    ) -> Result<u64, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let opened = crate::subscription_window_records::rescan_missing_windows_in_transaction(
            &mut transaction,
            &identity_key(reader),
            now,
        )
        .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(opened)
    }

    /// End every due subscription; off-only records are included.
    pub async fn end_expired_subscriptions(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<u64, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        let ended_count = end_expired_rows(&mut transaction, now, None).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(ended_count)
    }

    /// Restore durable active/draining rows and recover windows interrupted in flight.
    pub async fn restore_active_and_draining(
        &mut self,
        now: DateTime<Utc>,
    ) -> Result<Vec<ThreadSubscriptionRecord>, BoardError> {
        let mut transaction = self
            .connection
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage_error)?;
        sqlx::query!(
            "UPDATE subscription_windows SET in_flight_through=NULL,residual_opened_at=NULL"
        )
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let reader_keys = sqlx::query_scalar!(
            "SELECT DISTINCT reader_key FROM thread_subscriptions \
             WHERE state IN ('active','draining') ORDER BY reader_key"
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut records = Vec::new();
        for reader_key in reader_keys {
            match restore_reader_records(&mut transaction, &reader_key, now).await {
                Ok(reader_records) => records.extend(reader_records),
                Err(error) if error.kind == BoardFailureKind::InvalidRecord => {
                    tracing::warn!(%error, "corrupt subscription reader skipped during restore");
                }
                Err(error) => return Err(error),
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(records)
    }
}

/// A corrupt row quarantines its reader for this restore; other readers continue.
async fn restore_reader_records(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    now: DateTime<Utc>,
) -> Result<Vec<ThreadSubscriptionRecord>, BoardError> {
    let _ = end_expired_rows(transaction, now, Some(reader_key)).await?;
    crate::subscription_window_records::rescan_missing_windows_in_transaction(
        transaction,
        reader_key,
        now,
    )
    .await?;
    let scopes = sqlx::query!(
        "SELECT scope_kind,scope_id FROM thread_subscriptions \
         WHERE reader_key=? AND state IN ('active','draining') \
         ORDER BY scope_kind,scope_id",
        reader_key,
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let mut records = Vec::with_capacity(scopes.len());
    for row in scopes {
        let scope = decode_subscription_scope(row.scope_kind.as_str(), row.scope_id)?;
        records.push(
            load_subscription_record(transaction, reader_key, &scope)
                .await?
                .ok_or_else(invalid_record)?,
        );
    }
    Ok(records)
}
