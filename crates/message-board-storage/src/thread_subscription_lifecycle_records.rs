//! Subscription end, expiry, coverage, and lifecycle hooks.
use crate::board_topic_records::{require_board, require_topic};
use crate::message_records::{activate_watch, require_thread};
use crate::storage_support::{
    BoardTransaction, invalid_record, recompute_project_unread, storage_error,
};
use crate::thread_subscription_records::{
    SubscriptionWrite, delete_scope_windows, encode_end_reason, encode_scope, expiry_at,
    policy_error, stored_generation, upsert_subscription,
};
use crate::thread_subscription_row_decoding::{
    corrupt_subscription, decode_thread_subscription_record, encode_utc_timestamp,
};
use crate::thread_subscription_row_reads::{
    decode_subscription_scope, load_subscription_record, load_subscription_row,
};
use chrono::{DateTime, Utc};
use message_board::*;

struct CoveringSubscriptionRow {
    scope: SubscriptionScope,
    mode: SubscriptionMode,
    when_idle: WhenIdle,
    quiet_seconds: u64,
    cap_seconds: u64,
    lifetime_seconds: u64,
    generation: SubscriptionGeneration,
    state: SubscriptionState,
}

struct ExpiredSubscriptionRow {
    reader_key: String,
    scope_kind: String,
    scope_id: String,
}
pub(crate) async fn end_subscription(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
    reason: EndReason,
    now: DateTime<Utc>,
) -> Result<bool, BoardError> {
    let Some(record) = load_subscription_record(transaction, reader_key, scope).await? else {
        return Ok(false);
    };
    if matches!(record.state(), SubscriptionState::Ended { .. }) {
        delete_scope_windows(transaction, reader_key, scope).await?;
        return Ok(true);
    }
    let (scope_kind, scope_id) = encode_scope(scope);
    let next_generation = stored_generation(record.generation().next().map_err(policy_error)?)?;
    sqlx::query!(
        "UPDATE thread_subscriptions SET state='ended',end_reason=?,ended_at=?,generation=? \
         WHERE reader_key=? AND scope_kind=? AND scope_id=? AND state<>'ended'",
        encode_end_reason(reason),
        encode_utc_timestamp(now),
        next_generation,
        reader_key,
        scope_kind,
        scope_id,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    delete_scope_windows(transaction, reader_key, scope).await?;
    Ok(true)
}

pub(super) async fn end_expired_rows(
    transaction: &mut BoardTransaction<'_>,
    now: DateTime<Utc>,
    reader_key_filter: Option<&str>,
) -> Result<u64, BoardError> {
    let now_text = encode_utc_timestamp(now);
    let expired_scopes = match reader_key_filter {
        Some(reader_key) => sqlx::query_as!(
            ExpiredSubscriptionRow,
            "SELECT reader_key,scope_kind,scope_id FROM thread_subscriptions \
                 WHERE reader_key=? AND state IN ('active','draining') AND expires_at<=? \
                 ORDER BY scope_kind,scope_id",
            reader_key,
            now_text,
        )
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage_error)?,
        None => sqlx::query_as!(
            ExpiredSubscriptionRow,
            "SELECT reader_key,scope_kind,scope_id FROM thread_subscriptions \
                 WHERE state IN ('active','draining') AND expires_at<=? \
                 ORDER BY reader_key,scope_kind,scope_id",
            now_text,
        )
        .fetch_all(&mut **transaction)
        .await
        .map_err(storage_error)?,
    };
    let expired_count = u64::try_from(expired_scopes.len()).map_err(|_| invalid_record())?;
    for expired in expired_scopes {
        let scope = decode_subscription_scope(expired.scope_kind.as_str(), expired.scope_id)?;
        end_subscription(
            transaction,
            expired.reader_key.as_str(),
            &scope,
            EndReason::Expired,
            now,
        )
        .await?;
    }
    Ok(expired_count)
}

pub(crate) async fn renew_covering_subscription(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    let Some(covering) =
        load_covering_subscription(transaction, reader_key, root_message_id, now).await?
    else {
        return Ok(());
    };
    let expires_at = expiry_at(
        now,
        SubscriptionLifetime::new(covering.lifetime_seconds)
            .map_err(|error| corrupt_subscription(error.field))?,
    )?;
    let (scope_kind, scope_id) = encode_scope(&covering.scope);
    sqlx::query!(
        "UPDATE thread_subscriptions SET renewed_at=?,expires_at=? \
         WHERE reader_key=? AND scope_kind=? AND scope_id=? AND generation=? \
           AND state IN ('active','draining')",
        encode_utc_timestamp(now),
        encode_utc_timestamp(expires_at),
        reader_key,
        scope_kind,
        scope_id,
        stored_generation(covering.generation)?,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn load_covering_subscription(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<Option<CoveringSubscriptionRow>, BoardError> {
    let thread_scope = SubscriptionScope::thread(root_message_id.clone());
    if let Some(row) = load_subscription_row(transaction, reader_key, &thread_scope).await? {
        let decoded = decode_thread_subscription_record(row, Vec::new())?;
        if !decoded.state().is_delivery_eligible() || decoded.expires_at() <= now {
            return Ok(None);
        }
        return Ok(Some(CoveringSubscriptionRow {
            scope: decoded.scope().clone(),
            mode: decoded.policy().mode(),
            when_idle: decoded.policy().when_idle(),
            quiet_seconds: decoded.policy().timing().quiet_seconds(),
            cap_seconds: decoded.policy().timing().cap_seconds(),
            lifetime_seconds: decoded.policy().lifetime().seconds(),
            generation: decoded.generation(),
            state: decoded.state(),
        }));
    }
    let topic_id: String = sqlx::query_scalar!(
        "SELECT topic_id FROM board_messages WHERE message_id=? AND root_id IS NULL",
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?
    .ok_or_else(invalid_record)?;
    let topic_scope =
        SubscriptionScope::topic(TopicId::try_from(topic_id).map_err(|_| invalid_record())?);
    let Some(row) = load_subscription_row(transaction, reader_key, &topic_scope).await? else {
        return Ok(None);
    };
    let decoded = decode_thread_subscription_record(row, Vec::new())?;
    if decoded.state() != SubscriptionState::Active || decoded.expires_at() <= now {
        return Ok(None);
    }
    Ok(Some(CoveringSubscriptionRow {
        scope: decoded.scope().clone(),
        mode: decoded.policy().mode(),
        when_idle: decoded.policy().when_idle(),
        quiet_seconds: decoded.policy().timing().quiet_seconds(),
        cap_seconds: decoded.policy().timing().cap_seconds(),
        lifetime_seconds: decoded.policy().lifetime().seconds(),
        generation: decoded.generation(),
        state: decoded.state(),
    }))
}

pub(crate) async fn activate_topic_watch_scope(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    topic_id: &TopicId,
    boundary: i64,
) -> Result<(), BoardError> {
    let topic = require_topic(transaction, topic_id).await?;
    let board = require_board(transaction, &topic.board_id).await?;
    crate::storage_support::ensure_project_reader_state(
        transaction,
        reader_key,
        board.project_id.as_str(),
    )
    .await?;
    sqlx::query!(
        "INSERT INTO topic_watches(reader_key,topic_id,starts_after_activity,active) \
         VALUES(?,?,?,1) ON CONFLICT(reader_key,topic_id) DO UPDATE SET \
           starts_after_activity=CASE WHEN topic_watches.active=1 \
             THEN topic_watches.starts_after_activity ELSE excluded.starts_after_activity END, \
           active=1",
        reader_key,
        topic_id.as_str(),
        boundary,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    for root_id in sqlx::query_scalar!(
        "SELECT root.message_id FROM board_messages root \
         WHERE root.topic_id=? AND root.root_id IS NULL ORDER BY root.message_id",
        topic_id.as_str(),
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?
    {
        let root_id = MessageId::try_from(root_id).map_err(|_| invalid_record())?;
        activate_topic_root_watch(
            transaction,
            reader_key,
            &board.project_id,
            topic_id,
            &root_id,
            boundary,
        )
        .await?;
    }
    recompute_project_unread(transaction, reader_key, board.project_id.as_str()).await?;
    Ok(())
}

pub(crate) async fn ensure_topic_root_watch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &ProjectId,
    topic_id: &TopicId,
    root_message_id: &MessageId,
    boundary: i64,
) -> Result<(), BoardError> {
    if !topic_root_watch_can_be_materialized(transaction, reader_key, topic_id, root_message_id)
        .await?
    {
        return Ok(());
    }
    crate::storage_support::ensure_project_reader_state(
        transaction,
        reader_key,
        project_id.as_str(),
    )
    .await?;
    sqlx::query!(
        "INSERT INTO thread_watches(reader_key,root_id,active,starts_after_activity) \
         VALUES(?,?,1,?) ON CONFLICT(reader_key,root_id) DO NOTHING",
        reader_key,
        root_message_id.as_str(),
        boundary,
    )
    .execute(&mut **transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

async fn activate_topic_root_watch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    project_id: &ProjectId,
    topic_id: &TopicId,
    root_message_id: &MessageId,
    boundary: i64,
) -> Result<(), BoardError> {
    if !topic_root_watch_can_be_materialized(transaction, reader_key, topic_id, root_message_id)
        .await?
    {
        return Ok(());
    }
    crate::message_records::activate_watch(
        transaction,
        reader_key,
        project_id,
        root_message_id,
        boundary,
    )
    .await
}

async fn topic_root_watch_can_be_materialized(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    topic_id: &TopicId,
    root_message_id: &MessageId,
) -> Result<bool, BoardError> {
    let thread_override = sqlx::query_scalar!(
        "SELECT EXISTS(SELECT 1 FROM thread_subscriptions \
         WHERE reader_key=? AND scope_kind='thread' AND scope_id=?)",
        reader_key,
        root_message_id.as_str(),
    )
    .fetch_one(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if thread_override != 0 {
        return Ok(false);
    }
    let root_topic: Option<String> = sqlx::query_scalar!(
        "SELECT topic_id FROM board_messages WHERE message_id=? AND root_id IS NULL",
        root_message_id.as_str(),
    )
    .fetch_optional(&mut **transaction)
    .await
    .map_err(storage_error)?;
    if root_topic.as_deref() != Some(topic_id.as_str()) {
        return Err(invalid_record());
    }
    Ok(true)
}

pub(super) async fn activate_scope_watch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    scope: &SubscriptionScope,
    boundary: i64,
) -> Result<(), BoardError> {
    match scope {
        SubscriptionScope::Thread { root_message_id } => {
            let location = require_thread(transaction, root_message_id).await?;
            activate_watch(
                transaction,
                reader_key,
                &location.project_id,
                root_message_id,
                boundary,
            )
            .await
        }
        SubscriptionScope::Topic { topic_id } => {
            activate_topic_watch_scope(transaction, reader_key, topic_id, boundary).await
        }
    }
}

pub(crate) async fn upsert_join_subscription(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    reader: &Identity,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<bool, BoardError> {
    let scope = SubscriptionScope::thread(root_message_id.clone());
    let existing = load_subscription_record(transaction, reader_key, &scope).await?;
    let policy = existing
        .as_ref()
        .map(|record| record.policy().clone())
        .unwrap_or_else(|| SubscriptionPolicy::defaults_for(reader));
    let subscription_started = existing
        .as_ref()
        .is_none_or(|record| !matches!(record.state(), SubscriptionState::Active));
    let generation = match existing.as_ref() {
        None => SubscriptionGeneration::new(1).map_err(policy_error)?,
        Some(record) if subscription_started => record.generation().next().map_err(policy_error)?,
        Some(record) => record.generation(),
    };
    let previous_outcome = existing
        .as_ref()
        .and_then(ThreadSubscriptionRecord::last_outcome);
    upsert_subscription(
        transaction,
        SubscriptionWrite {
            reader_key,
            scope: &scope,
            policy: &policy,
            state: SubscriptionState::Active,
            now,
            generation,
            last_outcome: previous_outcome,
        },
    )
    .await?;
    Ok(subscription_started)
}

pub(crate) async fn end_thread_subscription_for_join_without_watch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    reader: &Identity,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    cancel_thread_subscription_for_unwatch(transaction, reader_key, reader, root_message_id, now)
        .await?;
    Ok(())
}

pub(crate) async fn end_thread_subscription_for_unwatch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    reader: &Identity,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    cancel_thread_subscription_for_unwatch(transaction, reader_key, reader, root_message_id, now)
        .await
}

async fn cancel_thread_subscription_for_unwatch(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    reader: &Identity,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    let scope = SubscriptionScope::thread(root_message_id.clone());
    if load_subscription_record(transaction, reader_key, &scope)
        .await?
        .is_some()
    {
        end_subscription(transaction, reader_key, &scope, EndReason::Cancelled, now).await?;
        return Ok(());
    }
    let Some(topic_coverage) =
        load_covering_subscription(transaction, reader_key, root_message_id, now).await?
    else {
        return Ok(());
    };
    if !matches!(topic_coverage.scope, SubscriptionScope::Topic { .. }) {
        return Ok(());
    }
    let policy = SubscriptionPolicy::defaults_for(reader);
    let generation = SubscriptionGeneration::new(1).map_err(policy_error)?;
    upsert_subscription(
        transaction,
        SubscriptionWrite {
            reader_key,
            scope: &scope,
            policy: &policy,
            state: SubscriptionState::Ended {
                reason: EndReason::Cancelled,
            },
            now,
            generation,
            last_outcome: None,
        },
    )
    .await
}

pub(crate) async fn end_thread_subscription_for_leave(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    reason: EndReason,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    end_subscription(
        transaction,
        reader_key,
        &SubscriptionScope::thread(root_message_id.clone()),
        reason,
        now,
    )
    .await?;
    Ok(())
}

pub(crate) async fn resolve_thread_subscriptions(
    transaction: &mut BoardTransaction<'_>,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<(), BoardError> {
    let reader_keys = sqlx::query_scalar!(
        "SELECT reader_key FROM thread_subscriptions \
         WHERE scope_kind='thread' AND scope_id=? AND state IN ('active','draining') \
         ORDER BY reader_key",
        root_message_id.as_str(),
    )
    .fetch_all(&mut **transaction)
    .await
    .map_err(storage_error)?;
    let scope = SubscriptionScope::thread(root_message_id.clone());
    for reader_key in reader_keys {
        let record = load_subscription_record(transaction, reader_key.as_str(), &scope)
            .await?
            .ok_or_else(invalid_record)?;
        if record.policy().mode() == SubscriptionMode::Off {
            end_subscription(
                transaction,
                reader_key.as_str(),
                &scope,
                EndReason::Resolved,
                now,
            )
            .await?;
        } else if record.policy().mode() == SubscriptionMode::Deliver
            || record.policy().mode() == SubscriptionMode::Poll
        {
            let next_generation =
                stored_generation(record.generation().next().map_err(policy_error)?)?;
            sqlx::query!(
                "UPDATE thread_subscriptions SET state='draining',generation=? \
                 WHERE reader_key=? AND scope_kind='thread' AND scope_id=? \
                   AND state IN ('active','draining')",
                next_generation,
                reader_key,
                root_message_id.as_str(),
            )
            .execute(&mut **transaction)
            .await
            .map_err(storage_error)?;
        } else {
            return Err(corrupt_subscription("mode"));
        }
    }
    Ok(())
}
pub(crate) async fn get_covering_subscription(
    transaction: &mut BoardTransaction<'_>,
    reader_key: &str,
    root_message_id: &MessageId,
    now: DateTime<Utc>,
) -> Result<Option<CoveringSubscription>, BoardError> {
    Ok(
        load_covering_subscription(transaction, reader_key, root_message_id, now)
            .await?
            .map(|row| CoveringSubscription {
                scope: row.scope,
                mode: row.mode,
                when_idle: row.when_idle,
                quiet_seconds: row.quiet_seconds,
                cap_seconds: row.cap_seconds,
                generation: row.generation,
                state: row.state,
            }),
    )
}

pub(crate) struct CoveringSubscription {
    pub scope: SubscriptionScope,
    pub mode: SubscriptionMode,
    pub when_idle: WhenIdle,
    pub quiet_seconds: u64,
    pub cap_seconds: u64,
    pub generation: SubscriptionGeneration,
    pub state: SubscriptionState,
}
