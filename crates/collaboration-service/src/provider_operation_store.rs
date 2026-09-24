//! Metadata-only persistence for Host-owned external provider operations.
//!
//! This store intentionally has no request, prompt, reply, diagnostic, or transcript input.

use collaboration_protocol::{
    OperationId, ProviderBindingIdentity, ProviderOperationEffect, ProviderOperationKind,
    ProviderOperationStage, ProviderReconciliationState, SessionRef,
};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{
    Connection, Row, SqliteConnection,
    migrate::MigrateError,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};
use std::{collections::HashSet, path::Path, time::Duration};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");
const RETENTION_BATCH_LIMIT: i64 = 1_000;
const EXPECTED_COLUMNS: [&str; 13] = [
    "operation_id",
    "operation_kind",
    "binding_json",
    "target_service_id",
    "target_endpoint_id",
    "target_session_id",
    "stage",
    "effect",
    "reconciliation_state",
    "admitted_at_ms",
    "dispatched_at_ms",
    "terminal_at_ms",
    "updated_at_ms",
];

#[derive(Debug, thiserror::Error)]
pub enum ProviderOperationStoreError {
    #[error("provider operation storage unavailable")]
    Database(#[from] sqlx::Error),
    #[error("provider operation migration failed")]
    Migration(#[from] MigrateError),
    #[error("provider operation identity is invalid")]
    InvalidIdentity,
    #[error("provider operation record is invalid")]
    InvalidRecord,
    #[error("provider operation was not found")]
    NotFound,
    #[error("provider operation state does not permit this transition")]
    TransitionConflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOperationAdmission {
    pub operation_id: OperationId,
    pub operation_kind: ProviderOperationKind,
    pub binding: ProviderBindingIdentity,
    pub admitted_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderOperationRecord {
    pub operation_id: OperationId,
    pub operation_kind: ProviderOperationKind,
    pub binding: ProviderBindingIdentity,
    pub target: Option<SessionRef>,
    pub stage: ProviderOperationStage,
    pub effect: ProviderOperationEffect,
    pub reconciliation_state: ProviderReconciliationState,
    pub admitted_at_ms: i64,
    pub dispatched_at_ms: Option<i64>,
    pub terminal_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderOperationAdmissionResult {
    Admitted(ProviderOperationRecord),
    Existing(ProviderOperationRecord),
}

pub struct ProviderOperationStore {
    pub(crate) connection: SqliteConnection,
}

struct StoredProviderOperationRow {
    operation_id: String,
    operation_kind: String,
    binding_json: String,
    target_service_id: Option<String>,
    target_endpoint_id: Option<String>,
    target_session_id: Option<String>,
    stage: String,
    effect: String,
    reconciliation_state: String,
    admitted_at_ms: i64,
    dispatched_at_ms: Option<i64>,
    terminal_at_ms: Option<i64>,
    updated_at_ms: i64,
}

impl ProviderOperationStore {
    pub async fn open(path: &Path) -> Result<Self, ProviderOperationStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .busy_timeout(Duration::from_secs(1));
        let mut connection = SqliteConnection::connect_with(&options).await?;
        let mut transaction = connection.begin_with("BEGIN IMMEDIATE").await?;
        MIGRATOR.run_direct(None, &mut *transaction, false).await?;
        transaction.commit().await?;
        validate_schema(&mut connection).await?;
        Ok(Self { connection })
    }

    pub async fn close(self) -> Result<(), ProviderOperationStoreError> {
        self.connection.close().await?;
        Ok(())
    }

    pub async fn admit(
        &mut self,
        admission: ProviderOperationAdmission,
    ) -> Result<ProviderOperationAdmissionResult, ProviderOperationStoreError> {
        let operation_kind = encode_enum(admission.operation_kind)?;
        let binding_json = encode_closed(&admission.binding)?;
        let result = sqlx::query!(
            "INSERT OR IGNORE INTO provider_operations (
                operation_id,operation_kind,binding_json,
                stage,effect,reconciliation_state,admitted_at_ms,updated_at_ms
             ) VALUES (?,?,?,?,?,?,?,?)",
            admission.operation_id.as_str(),
            operation_kind,
            binding_json,
            encode_enum(ProviderOperationStage::Admitted)?,
            encode_enum(ProviderOperationEffect::None)?,
            encode_enum(ProviderReconciliationState::Unresolved)?,
            admission.admitted_at_ms,
            admission.admitted_at_ms,
        )
        .execute(&mut self.connection)
        .await?;
        let record = self
            .inspect(&admission.operation_id)
            .await?
            .ok_or(ProviderOperationStoreError::InvalidRecord)?;
        if result.rows_affected() == 1 {
            Ok(ProviderOperationAdmissionResult::Admitted(record))
        } else {
            Ok(ProviderOperationAdmissionResult::Existing(record))
        }
    }

    pub async fn inspect(
        &mut self,
        operation_id: &OperationId,
    ) -> Result<Option<ProviderOperationRecord>, ProviderOperationStoreError> {
        sqlx::query_as!(
            StoredProviderOperationRow,
            "SELECT operation_id,operation_kind,binding_json,
                    target_service_id,target_endpoint_id,target_session_id,
                    stage,effect,reconciliation_state,admitted_at_ms,dispatched_at_ms,
                    terminal_at_ms,updated_at_ms
             FROM provider_operations WHERE operation_id=?",
            operation_id.as_str(),
        )
        .fetch_optional(&mut self.connection)
        .await?
        .map(decode_record)
        .transpose()
    }

    pub async fn has_blocking_uncertainty(
        &mut self,
        binding: &ProviderBindingIdentity,
        target: Option<&SessionRef>,
    ) -> Result<bool, ProviderOperationStoreError> {
        let unknown = encode_enum(ProviderOperationEffect::Unknown)?;
        let candidates = sqlx::query!(
            "SELECT binding_json,target_service_id,target_endpoint_id,target_session_id
             FROM provider_operations WHERE effect=?",
            unknown,
        )
        .fetch_all(&mut self.connection)
        .await?;
        for candidate in candidates {
            let stored_binding: ProviderBindingIdentity = decode_closed(&candidate.binding_json)?;
            let same_scope = stored_binding.endpoint == binding.endpoint
                && stored_binding.runtime.provider == binding.runtime.provider;
            if !same_scope {
                continue;
            }
            let blocks = match (
                target,
                candidate.target_service_id,
                candidate.target_endpoint_id,
                candidate.target_session_id,
            ) {
                (_, None, None, None) => true,
                (Some(target), Some(service_id), Some(endpoint_id), Some(session_id)) => {
                    service_id == String::from(target.endpoint.service_id.clone())
                        && endpoint_id == String::from(target.endpoint.endpoint_id.clone())
                        && session_id == String::from(target.session_id.clone())
                }
                (None, Some(_), Some(_), Some(_)) => false,
                _ => return Err(ProviderOperationStoreError::InvalidRecord),
            };
            if blocks {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub async fn mark_may_have_dispatched(
        &mut self,
        operation_id: &OperationId,
        dispatched_at_ms: i64,
    ) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
        let result = sqlx::query!(
            "UPDATE provider_operations
             SET stage=?,effect=?,dispatched_at_ms=MAX(admitted_at_ms,?),updated_at_ms=MAX(updated_at_ms,admitted_at_ms,?)
             WHERE operation_id=? AND stage=?",
             encode_enum(ProviderOperationStage::MayHaveDispatched)?,
             encode_enum(ProviderOperationEffect::Unknown)?, dispatched_at_ms,
             dispatched_at_ms, operation_id.as_str(),
             encode_enum(ProviderOperationStage::Admitted)?,
        )
        .execute(&mut self.connection)
        .await?;
        self.transition_result(operation_id, result.rows_affected(), |record| {
            record.stage == ProviderOperationStage::MayHaveDispatched
                && record.dispatched_at_ms.is_some()
        })
        .await
    }

    pub async fn record_target(
        &mut self,
        operation_id: &OperationId,
        target: &SessionRef,
        observed_at_ms: i64,
    ) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
        let target_service_id = String::from(target.endpoint.service_id.clone());
        let target_endpoint_id = String::from(target.endpoint.endpoint_id.clone());
        let target_session_id = String::from(target.session_id.clone());
        let result = sqlx::query!(
            "UPDATE provider_operations
             SET target_service_id=?,target_endpoint_id=?,target_session_id=?,updated_at_ms=MAX(updated_at_ms,admitted_at_ms,?)
             WHERE operation_id=? AND (
                target_session_id IS NULL OR (
                    target_service_id=? AND target_endpoint_id=? AND target_session_id=?
                )
             )",
             target_service_id, target_endpoint_id, target_session_id, observed_at_ms,
             operation_id.as_str(), target_service_id, target_endpoint_id, target_session_id,
        )
        .execute(&mut self.connection)
        .await?;
        self.transition_result(operation_id, result.rows_affected(), |record| {
            record.target.as_ref() == Some(target)
        })
        .await
    }

    pub async fn record_terminal(
        &mut self,
        operation_id: &OperationId,
        effect: ProviderOperationEffect,
        reconciliation_state: ProviderReconciliationState,
        terminal_at_ms: i64,
    ) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
        let result = sqlx::query!(
            "UPDATE provider_operations
             SET stage=?,effect=?,reconciliation_state=?,terminal_at_ms=MAX(admitted_at_ms,?),updated_at_ms=MAX(updated_at_ms,admitted_at_ms,?)
             WHERE operation_id=? AND stage!=?",
             encode_enum(ProviderOperationStage::Terminal)?, encode_enum(effect)?,
             encode_enum(reconciliation_state)?, terminal_at_ms, terminal_at_ms,
             operation_id.as_str(), encode_enum(ProviderOperationStage::Terminal)?,
        )
        .execute(&mut self.connection)
        .await?;
        self.transition_result(operation_id, result.rows_affected(), |record| {
            record.stage == ProviderOperationStage::Terminal
                && record.effect == effect
                && record.reconciliation_state == reconciliation_state
                && record.terminal_at_ms.is_some()
        })
        .await
    }

    pub async fn record_not_reconcilable(
        &mut self,
        operation_id: &OperationId,
        observed_at_ms: i64,
    ) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
        let result = sqlx::query!(
            "UPDATE provider_operations
             SET stage=?,reconciliation_state=?,terminal_at_ms=COALESCE(terminal_at_ms,MAX(admitted_at_ms,?)),updated_at_ms=MAX(updated_at_ms,admitted_at_ms,?)
             WHERE operation_id=? AND reconciliation_state=?",
             encode_enum(ProviderOperationStage::Terminal)?,
             encode_enum(ProviderReconciliationState::NotReconcilable)?, observed_at_ms,
             observed_at_ms, operation_id.as_str(),
             encode_enum(ProviderReconciliationState::Unresolved)?,
        )
        .execute(&mut self.connection)
        .await?;
        self.transition_result(operation_id, result.rows_affected(), |record| {
            record.stage == ProviderOperationStage::Terminal
                && record.reconciliation_state == ProviderReconciliationState::NotReconcilable
                && record.terminal_at_ms.is_some()
        })
        .await
    }

    pub async fn prune_terminal_before(
        &mut self,
        cutoff_ms: i64,
        protected_operation_ids: &HashSet<OperationId>,
    ) -> Result<u64, ProviderOperationStoreError> {
        let candidates = sqlx::query_scalar!(
            "SELECT operation_id FROM provider_operations
             WHERE stage=? AND reconciliation_state!=? AND effect!=? AND terminal_at_ms<?
             ORDER BY terminal_at_ms,operation_id LIMIT ?",
            encode_enum(ProviderOperationStage::Terminal)?,
            encode_enum(ProviderReconciliationState::Unresolved)?,
            encode_enum(ProviderOperationEffect::Unknown)?,
            cutoff_ms,
            RETENTION_BATCH_LIMIT,
        )
        .fetch_all(&mut self.connection)
        .await?;
        let mut transaction = self.connection.begin().await?;
        let mut removed = 0_u64;
        for operation_id in candidates {
            let typed_id = OperationId::try_from(operation_id.clone())
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
            if protected_operation_ids.contains(&typed_id) {
                continue;
            }
            removed = removed.saturating_add(
                sqlx::query!(
                    "DELETE FROM provider_operations
                     WHERE operation_id=? AND stage=? AND reconciliation_state!=? AND effect!=? AND terminal_at_ms<?",
                    operation_id,
                    encode_enum(ProviderOperationStage::Terminal)?,
                    encode_enum(ProviderReconciliationState::Unresolved)?,
                    encode_enum(ProviderOperationEffect::Unknown)?,
                    cutoff_ms,
                )
                .execute(&mut *transaction)
                .await?
                .rows_affected(),
            );
        }
        transaction.commit().await?;
        Ok(removed)
    }

    async fn transition_result(
        &mut self,
        operation_id: &OperationId,
        rows_affected: u64,
        matches_requested_state: impl FnOnce(&ProviderOperationRecord) -> bool,
    ) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
        let record = self
            .inspect(operation_id)
            .await?
            .ok_or(ProviderOperationStoreError::NotFound)?;
        if rows_affected == 1 || matches_requested_state(&record) {
            Ok(record)
        } else {
            Err(ProviderOperationStoreError::TransitionConflict)
        }
    }
}

fn decode_record(
    row: StoredProviderOperationRow,
) -> Result<ProviderOperationRecord, ProviderOperationStoreError> {
    let target_parts = (
        row.target_service_id,
        row.target_endpoint_id,
        row.target_session_id,
    );
    let target = match target_parts {
        (None, None, None) => None,
        (Some(service_id), Some(endpoint_id), Some(session_id)) => Some(SessionRef {
            endpoint: collaboration_protocol::EndpointRef {
                service_id: collaboration_protocol::UuidIdentity::try_from(service_id)
                    .map_err(|_| ProviderOperationStoreError::InvalidRecord)?,
                endpoint_id: collaboration_protocol::EndpointId::try_from(endpoint_id)
                    .map_err(|_| ProviderOperationStoreError::InvalidRecord)?,
            },
            session_id: collaboration_protocol::SessionId::try_from(session_id)
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?,
        }),
        _ => return Err(ProviderOperationStoreError::InvalidRecord),
    };
    let record = ProviderOperationRecord {
        operation_id: OperationId::try_from(row.operation_id)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?,
        operation_kind: decode_enum(row.operation_kind)?,
        binding: decode_closed(&row.binding_json)?,
        target,
        stage: decode_enum(row.stage)?,
        effect: decode_enum(row.effect)?,
        reconciliation_state: decode_enum(row.reconciliation_state)?,
        admitted_at_ms: row.admitted_at_ms,
        dispatched_at_ms: row.dispatched_at_ms,
        terminal_at_ms: row.terminal_at_ms,
        updated_at_ms: row.updated_at_ms,
    };
    validate_record(&record)?;
    Ok(record)
}

pub(crate) fn encode_enum<T: Serialize>(value: T) -> Result<String, ProviderOperationStoreError> {
    let value =
        serde_json::to_value(value).map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or(ProviderOperationStoreError::InvalidRecord)
}

fn decode_enum<T: DeserializeOwned>(value: String) -> Result<T, ProviderOperationStoreError> {
    serde_json::from_value(serde_json::Value::String(value))
        .map_err(|_| ProviderOperationStoreError::InvalidRecord)
}

fn encode_closed<T: Serialize>(value: &T) -> Result<String, ProviderOperationStoreError> {
    serde_json::to_string(value).map_err(|_| ProviderOperationStoreError::InvalidRecord)
}

fn decode_closed<T: DeserializeOwned>(value: &str) -> Result<T, ProviderOperationStoreError> {
    serde_json::from_str(value).map_err(|_| ProviderOperationStoreError::InvalidRecord)
}

fn validate_record(record: &ProviderOperationRecord) -> Result<(), ProviderOperationStoreError> {
    let timestamps_valid = record.updated_at_ms >= record.admitted_at_ms
        && record
            .dispatched_at_ms
            .is_none_or(|value| value >= record.admitted_at_ms && value <= record.updated_at_ms)
        && record
            .terminal_at_ms
            .is_none_or(|value| value >= record.admitted_at_ms && value <= record.updated_at_ms);
    let state_valid = match record.stage {
        ProviderOperationStage::Admitted => {
            record.effect == ProviderOperationEffect::None
                && record.dispatched_at_ms.is_none()
                && record.terminal_at_ms.is_none()
        }
        ProviderOperationStage::MayHaveDispatched => {
            record.effect == ProviderOperationEffect::Unknown
                && record.dispatched_at_ms.is_some()
                && record.terminal_at_ms.is_none()
        }
        ProviderOperationStage::Terminal => record.terminal_at_ms.is_some(),
    };
    if timestamps_valid && state_valid {
        Ok(())
    } else {
        Err(ProviderOperationStoreError::InvalidRecord)
    }
}

async fn validate_schema(
    connection: &mut SqliteConnection,
) -> Result<(), ProviderOperationStoreError> {
    // SQLite PRAGMA metadata has no stable SQLx compile-time column mapping;
    // every state-changing and state-reading provider query is checked above.
    let columns = sqlx::query("PRAGMA table_info(provider_operations)")
        .fetch_all(connection)
        .await?
        .into_iter()
        .map(|row| row.try_get::<String, _>("name"))
        .collect::<Result<Vec<_>, _>>()?;
    if columns.iter().map(String::as_str).eq(EXPECTED_COLUMNS) {
        Ok(())
    } else {
        Err(ProviderOperationStoreError::InvalidRecord)
    }
}
