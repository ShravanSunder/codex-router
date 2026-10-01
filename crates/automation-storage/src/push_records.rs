//! Durable push snapshots, delivery transitions, DM queries, and bounded pruning.
use crate::{AutomationStore, StorageError, push_record_rows};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use collaboration_protocol::{
    DeliveryOutcome, DeliveryReceipt, PushDeliveryState, PushId, PushKind, PushOrigin, PushRecord,
    PushRecordDraft, RouterOriginRef, SessionRef,
};
use sqlx::SqliteConnection;
use std::time::Duration;

const PUSH_INSERT_RETRY_WINDOW: Duration = Duration::from_secs(5);
const PUSH_CONNECTION_BUSY_TIMEOUT: Duration = Duration::from_secs(1);
const PUSH_INSERT_INITIAL_BACKOFF: Duration = Duration::from_millis(25);
const PUSH_INSERT_MAX_BACKOFF: Duration = Duration::from_millis(250);
const SQLITE_BUSY_PRIMARY_CODE: i32 = 5;
const SQLITE_PRIMARY_CODE_MASK: i32 = 0xff;
pub const MAX_PUSH_PRUNE_BATCH: u32 = 500;
pub const MAX_PUSH_LIST_LIMIT: u32 = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PushInboxQuery {
    pub target: SessionRef,
    pub limit: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectMessageHistoryQuery {
    pub caller: SessionRef,
    pub with: SessionRef,
    pub limit: u32,
}

impl AutomationStore {
    /// Persists a validated, pending push before its delivery is attempted.
    pub async fn insert_push_record(
        &mut self,
        draft: PushRecordDraft,
    ) -> Result<PushRecord, StorageError> {
        let record = draft
            .into_pending()
            .map_err(|_| StorageError::InvalidRecord)?;
        let deadline = tokio::time::Instant::now() + PUSH_INSERT_RETRY_WINDOW;
        let mut backoff = PUSH_INSERT_INITIAL_BACKOFF;

        loop {
            match self.insert_push_record_once(&record).await {
                Ok(()) => return Ok(record),
                Err(error) if is_sqlite_busy(&error) => {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining <= PUSH_CONNECTION_BUSY_TIMEOUT {
                        return Err(error);
                    }
                    tokio::time::sleep(backoff.min(remaining)).await;
                    backoff = (backoff * 2).min(PUSH_INSERT_MAX_BACKOFF);
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn insert_push_record_once(&mut self, record: &PushRecord) -> Result<(), StorageError> {
        insert_push_record_with_connection(&mut self.connection, record).await
    }

    pub async fn mark_push_attempted(
        &mut self,
        push_id: &PushId,
    ) -> Result<PushRecord, StorageError> {
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='attempted',last_outcome_json=NULL,settled_at=NULL WHERE push_id=? AND delivery_state IN ('pending','held')",
            push_id.as_str()
        )
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            return self.push_transition_error(push_id).await;
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    pub async fn restore_push_pending_after_not_started(
        &mut self,
        push_id: &PushId,
    ) -> Result<PushRecord, StorageError> {
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='pending',last_outcome_json=NULL,settled_at=NULL WHERE push_id=? AND delivery_state='attempted'",
            push_id.as_str()
        )
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            return self.push_transition_error(push_id).await;
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    pub async fn hold_push_record(
        &mut self,
        push_id: &PushId,
        outcome: DeliveryReceipt,
    ) -> Result<PushRecord, StorageError> {
        if !matches!(
            outcome.outcome,
            DeliveryOutcome::NotSubmitted {
                retryable: true,
                ..
            }
        ) {
            return Err(StorageError::InvalidRecord);
        }
        let outcome_json = push_record_rows::serialize_json(&outcome)?;
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state='held',last_outcome_json=?,settled_at=NULL WHERE push_id=? AND delivery_state='attempted'",
            outcome_json,
            push_id.as_str()
        )
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            return self.push_transition_error(push_id).await;
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    pub async fn settle_push_record(
        &mut self,
        push_id: &PushId,
        outcome: DeliveryReceipt,
        settled_at: DateTime<Utc>,
    ) -> Result<PushRecord, StorageError> {
        let state = settled_state(&outcome.outcome);
        let outcome_json = push_record_rows::serialize_json(&outcome)?;
        let settled_at = push_record_rows::serialize_timestamp(settled_at);
        let result = sqlx::query!(
            "UPDATE router_pushes SET delivery_state=?,last_outcome_json=?,settled_at=? WHERE push_id=? AND delivery_state='attempted'",
            push_record_rows::serialize_delivery_state(state),
            outcome_json,
            settled_at,
            push_id.as_str()
        )
        .execute(&mut self.connection)
        .await?;
        if result.rows_affected() != 1 {
            return self.push_transition_error(push_id).await;
        }
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    pub async fn get_push_record(
        &mut self,
        push_id: &PushId,
    ) -> Result<Option<PushRecord>, StorageError> {
        let row = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE push_id=?",
            push_id.as_str()
        )
        .fetch_optional(&mut self.connection)
        .await?;
        row.map(push_record_rows::PushRecordRow::into_record)
            .transpose()
    }

    pub async fn get_push_record_by_router_ref(
        &mut self,
        origin_router_ref: &str,
    ) -> Result<Option<PushRecord>, StorageError> {
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE origin_kind='router' AND origin_router_ref=? ORDER BY created_at,push_id",
            origin_router_ref
        )
        .fetch_all(&mut self.connection)
        .await?;
        if rows.len() > 1 {
            return Err(StorageError::InvalidRecord);
        }
        rows.into_iter()
            .next()
            .map(push_record_rows::PushRecordRow::into_record)
            .transpose()
    }

    pub async fn get_push_record_by_origin_reference(
        &mut self,
        origin: &RouterOriginRef,
    ) -> Result<Option<PushRecord>, StorageError> {
        let origin_router_ref = origin
            .canonical_string()
            .map_err(|_| StorageError::InvalidRecord)?;
        let record = self
            .get_push_record_by_router_ref(&origin_router_ref)
            .await?;
        if record.as_ref().is_some_and(|record| {
            record.origin_router_ref.as_deref() != Some(origin_router_ref.as_str())
                || !origin.supports_kind(record.kind)
        }) {
            return Err(StorageError::InvalidRecord);
        }
        Ok(record)
    }

    pub async fn list_pending_push_records(
        &mut self,
        target: &SessionRef,
    ) -> Result<Vec<PushRecord>, StorageError> {
        let (service_id, endpoint_id, session_id) = session_key(target);
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND delivery_state='pending' ORDER BY created_at,push_id",
            service_id,
            endpoint_id,
            session_id
        )
        .fetch_all(&mut self.connection)
        .await?;
        rows.into_iter()
            .map(push_record_rows::PushRecordRow::into_record)
            .collect()
    }

    pub async fn list_held_push_records(
        &mut self,
        target: &SessionRef,
    ) -> Result<Vec<PushRecord>, StorageError> {
        let (service_id, endpoint_id, session_id) = session_key(target);
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND delivery_state='held' ORDER BY created_at,push_id",
            service_id,
            endpoint_id,
            session_id
        )
        .fetch_all(&mut self.connection)
        .await?;
        rows.into_iter()
            .map(push_record_rows::PushRecordRow::into_record)
            .collect()
    }

    pub async fn list_direct_message_inbox(
        &mut self,
        request: &PushInboxQuery,
    ) -> Result<Vec<PushRecord>, StorageError> {
        validate_limit(request.limit)?;
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let (service_id, endpoint_id, session_id) = session_key(&request.target);
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE kind=? AND target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND read_at IS NULL ORDER BY created_at DESC,push_id DESC LIMIT ?",
            kind,
            service_id,
            endpoint_id,
            session_id,
            i64::from(request.limit)
        )
        .fetch_all(&mut self.connection)
        .await?;
        rows.into_iter()
            .map(push_record_rows::PushRecordRow::into_record)
            .collect()
    }

    pub async fn list_direct_message_history(
        &mut self,
        request: &DirectMessageHistoryQuery,
    ) -> Result<Vec<PushRecord>, StorageError> {
        validate_limit(request.limit)?;
        let kind = push_record_rows::serialize_kind(PushKind::DirectMessage)?;
        let (caller_service_id, caller_endpoint_id, caller_session_id) =
            session_key(&request.caller);
        let (with_service_id, with_endpoint_id, with_session_id) = session_key(&request.with);
        let rows = sqlx::query_as!(
            push_record_rows::PushRecordRow,
            "SELECT push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at FROM router_pushes WHERE kind=? AND ((target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND origin_service_id=? AND origin_endpoint_id=? AND origin_session_id=?) OR (target_service_id=? AND target_endpoint_id=? AND target_session_id=? AND origin_service_id=? AND origin_endpoint_id=? AND origin_session_id=?)) ORDER BY created_at DESC,push_id DESC LIMIT ?",
            kind,
            caller_service_id,
            caller_endpoint_id,
            caller_session_id,
            with_service_id,
            with_endpoint_id,
            with_session_id,
            with_service_id,
            with_endpoint_id,
            with_session_id,
            caller_service_id,
            caller_endpoint_id,
            caller_session_id,
            i64::from(request.limit)
        )
        .fetch_all(&mut self.connection)
        .await?;
        rows.into_iter()
            .map(push_record_rows::PushRecordRow::into_record)
            .collect()
    }

    pub async fn mark_push_read(
        &mut self,
        push_id: &PushId,
        caller: &SessionRef,
        read_at: DateTime<Utc>,
    ) -> Result<PushRecord, StorageError> {
        let record = self
            .get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)?;
        if record.kind != PushKind::DirectMessage {
            if &record.target != caller {
                return Err(StorageError::PushNotPermitted);
            }
            return Ok(record);
        }
        if &record.target != caller {
            if record.origin == PushOrigin::Session(caller.clone()) {
                return Ok(record);
            }
            return Err(StorageError::PushNotPermitted);
        }
        let read_at = push_record_rows::serialize_timestamp(read_at);
        sqlx::query!(
            "UPDATE router_pushes SET read_at=COALESCE(read_at,?) WHERE push_id=?",
            read_at,
            push_id.as_str()
        )
        .execute(&mut self.connection)
        .await?;
        self.get_push_record(push_id)
            .await?
            .ok_or(StorageError::PushNotFound)
    }

    pub async fn prune_push_records(
        &mut self,
        now: DateTime<Utc>,
        batch_size: u32,
    ) -> Result<u64, StorageError> {
        if !(1..=MAX_PUSH_PRUNE_BATCH).contains(&batch_size) {
            return Err(StorageError::InvalidRecord);
        }
        let cutoff = now
            .checked_sub_signed(ChronoDuration::days(30))
            .ok_or(StorageError::InvalidRecord)?;
        let cutoff = push_record_rows::serialize_timestamp(cutoff);
        let result = sqlx::query!(
            "DELETE FROM router_pushes WHERE push_id IN (SELECT push_id FROM router_pushes WHERE created_at < ? ORDER BY created_at,push_id LIMIT ?)",
            cutoff,
            i64::from(batch_size)
        )
        .execute(&mut self.connection)
        .await?;
        Ok(result.rows_affected())
    }

    async fn push_transition_error<T>(&mut self, push_id: &PushId) -> Result<T, StorageError> {
        if self.get_push_record(push_id).await?.is_none() {
            Err(StorageError::PushNotFound)
        } else {
            Err(StorageError::PushStateConflict)
        }
    }
}

pub(crate) async fn insert_push_record_with_connection(
    connection: &mut SqliteConnection,
    record: &PushRecord,
) -> Result<(), StorageError> {
    push_record_rows::validate_router_origin_reference(
        record.kind,
        &record.origin,
        record.origin_router_ref.as_deref(),
    )?;
    let kind = push_record_rows::serialize_kind(record.kind)?;
    let (origin_kind, origin_session) = push_record_rows::serialize_origin(&record.origin);
    let origin_service_id =
        origin_session.map(|session| String::from(session.endpoint.service_id.clone()));
    let origin_endpoint_id =
        origin_session.map(|session| String::from(session.endpoint.endpoint_id.clone()));
    let origin_session_id = origin_session.map(|session| String::from(session.session_id.clone()));
    let target_service_id = String::from(record.target.endpoint.service_id.clone());
    let target_endpoint_id = String::from(record.target.endpoint.endpoint_id.clone());
    let target_session_id = String::from(record.target.session_id.clone());
    let reply_to_push_id = record
        .reply_to_push_id
        .as_ref()
        .map(|push_id| push_id.as_str());
    let header_facts_json = push_record_rows::serialize_json(&record.header_facts)?;
    let ranges_json = record
        .activity
        .as_ref()
        .map(push_record_rows::serialize_json)
        .transpose()?;
    let created_at = push_record_rows::serialize_timestamp(record.created_at);

    let result = sqlx::query!(
        "INSERT INTO router_pushes (push_id,kind,origin_kind,origin_service_id,origin_endpoint_id,origin_session_id,origin_router_ref,target_service_id,target_endpoint_id,target_session_id,reply_to_push_id,header_facts_json,body,ranges_json,delivery_state,last_outcome_json,created_at,settled_at,read_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,'pending',NULL,?,NULL,NULL) ON CONFLICT(push_id) DO NOTHING",
        record.push_id.as_str(),
        kind,
        origin_kind,
        origin_service_id,
        origin_endpoint_id,
        origin_session_id,
        &record.origin_router_ref,
        target_service_id,
        target_endpoint_id,
        target_session_id,
        reply_to_push_id,
        header_facts_json,
        record.body.as_deref(),
        ranges_json,
        created_at
    )
    .execute(connection)
    .await?;
    if result.rows_affected() == 0 {
        return Err(StorageError::PushAlreadyExists);
    }
    Ok(())
}

fn session_key(session: &SessionRef) -> (String, String, String) {
    (
        String::from(session.endpoint.service_id.clone()),
        String::from(session.endpoint.endpoint_id.clone()),
        String::from(session.session_id.clone()),
    )
}

fn validate_limit(limit: u32) -> Result<(), StorageError> {
    if !(1..=MAX_PUSH_LIST_LIMIT).contains(&limit) {
        Err(StorageError::InvalidRecord)
    } else {
        Ok(())
    }
}

fn settled_state(outcome: &DeliveryOutcome) -> PushDeliveryState {
    match outcome {
        DeliveryOutcome::Started
        | DeliveryOutcome::Steered
        | DeliveryOutcome::StartedOrSteered
        | DeliveryOutcome::Queued
        | DeliveryOutcome::PeerMessageWritten => PushDeliveryState::Delivered,
        DeliveryOutcome::Unknown => PushDeliveryState::OutcomeUnknown,
        DeliveryOutcome::NotSubmitted { .. } | DeliveryOutcome::Rejected(_) => {
            PushDeliveryState::Rejected
        }
    }
}

fn is_sqlite_busy(error: &StorageError) -> bool {
    let StorageError::Database(sqlx_error) = error else {
        return false;
    };
    sqlx_error
        .as_database_error()
        .and_then(|database_error| database_error.code())
        .and_then(|code| code.parse::<i32>().ok())
        .is_some_and(|code| code & SQLITE_PRIMARY_CODE_MASK == SQLITE_BUSY_PRIMARY_CODE)
}
