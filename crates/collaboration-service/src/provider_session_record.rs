//! Durable provider session metadata needed for load-on-demand and approvals.
use crate::{ProviderOperationStore, ProviderOperationStoreError};
use collaboration_protocol::{
    OperationId, ProviderOperationEffect, ProviderOperationKind, ProviderOperationStage,
    ProviderReconciliationState, ProviderRequestedPolicy, ProviderWorkingDirectory, SessionRef,
};
use sqlx::Connection;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSessionRecord {
    pub target: SessionRef,
    pub working_directory: ProviderWorkingDirectory,
    pub requested_policy: ProviderRequestedPolicy,
    pub created_by: SessionRef,
    pub approver: SessionRef,
    pub updated_at_ms: i64,
}

impl ProviderSessionRecord {
    fn validate(&self) -> Result<(), ProviderOperationStoreError> {
        let endpoint_id = String::from(self.target.endpoint.endpoint_id.clone());
        if !matches!(endpoint_id.as_str(), "claude-local" | "cursor-local")
            || self.created_by.endpoint.service_id != self.target.endpoint.service_id
            || self.approver.endpoint.service_id != self.target.endpoint.service_id
            || self.updated_at_ms < 0
        {
            return Err(ProviderOperationStoreError::InvalidRecord);
        }
        Ok(())
    }
}

impl ProviderOperationStore {
    /// Atomically settle create/load and make its target loadable after a restart.
    pub async fn settle_session_operation(
        &mut self,
        operation_id: &OperationId,
        record: &ProviderSessionRecord,
    ) -> Result<(), ProviderOperationStoreError> {
        record.validate()?;
        let operation = self
            .inspect(operation_id)
            .await?
            .ok_or(ProviderOperationStoreError::NotFound)?;
        if operation.binding.endpoint != record.target.endpoint {
            return Err(ProviderOperationStoreError::InvalidRecord);
        }
        let target_service_id = String::from(record.target.endpoint.service_id.clone());
        let target_endpoint_id = String::from(record.target.endpoint.endpoint_id.clone());
        let target_session_id = String::from(record.target.session_id.clone());
        let working_directory = String::from(record.working_directory.clone());
        let requested_policy_json = serde_json::to_string(&record.requested_policy)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        let created_by_json = serde_json::to_string(&record.created_by)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        let approver_json = serde_json::to_string(&record.approver)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        let mut transaction = self.connection.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query!(
            "UPDATE provider_operations SET
                target_service_id=?,target_endpoint_id=?,target_session_id=?,
                stage=?,effect=?,reconciliation_state=?,
                terminal_at_ms=MAX(admitted_at_ms,?),
                updated_at_ms=MAX(updated_at_ms,admitted_at_ms,?)
             WHERE operation_id=? AND stage=? AND operation_kind IN (?,?)
             AND (target_session_id IS NULL OR
                 (target_service_id=? AND target_endpoint_id=? AND target_session_id=?))",
            target_service_id,
            target_endpoint_id,
            target_session_id,
            super::provider_operation_store::encode_enum(ProviderOperationStage::Terminal)?,
            super::provider_operation_store::encode_enum(ProviderOperationEffect::Applied)?,
            super::provider_operation_store::encode_enum(ProviderReconciliationState::Confirmed)?,
            record.updated_at_ms,
            record.updated_at_ms,
            operation_id.as_str(),
            super::provider_operation_store::encode_enum(
                ProviderOperationStage::MayHaveDispatched
            )?,
            super::provider_operation_store::encode_enum(
                ProviderOperationKind::ConversationCreate
            )?,
            super::provider_operation_store::encode_enum(ProviderOperationKind::ConversationLoad)?,
            target_service_id,
            target_endpoint_id,
            target_session_id,
        )
        .execute(&mut *transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(ProviderOperationStoreError::TransitionConflict);
        }
        sqlx::query!(
            "INSERT INTO provider_session_records (
                target_service_id,target_endpoint_id,target_session_id,
                working_directory,requested_policy_json,created_by_json,approver_json,updated_at_ms
            ) VALUES (?,?,?,?,?,?,?,?)
            ON CONFLICT(target_service_id,target_endpoint_id,target_session_id) DO UPDATE SET
                working_directory=excluded.working_directory,
                requested_policy_json=excluded.requested_policy_json,
                approver_json=excluded.approver_json,
                updated_at_ms=excluded.updated_at_ms",
            target_service_id,
            target_endpoint_id,
            target_session_id,
            working_directory,
            requested_policy_json,
            created_by_json,
            approver_json,
            record.updated_at_ms,
        )
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(())
    }

    pub async fn record_session(
        &mut self,
        record: &ProviderSessionRecord,
    ) -> Result<(), ProviderOperationStoreError> {
        record.validate()?;
        let target_service_id = String::from(record.target.endpoint.service_id.clone());
        let target_endpoint_id = String::from(record.target.endpoint.endpoint_id.clone());
        let target_session_id = String::from(record.target.session_id.clone());
        let working_directory = String::from(record.working_directory.clone());
        let requested_policy_json = serde_json::to_string(&record.requested_policy)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        let created_by_json = serde_json::to_string(&record.created_by)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        let approver_json = serde_json::to_string(&record.approver)
            .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
        sqlx::query!(
            "INSERT INTO provider_session_records (
                target_service_id,target_endpoint_id,target_session_id,
                working_directory,requested_policy_json,created_by_json,approver_json,updated_at_ms
            ) VALUES (?,?,?,?,?,?,?,?)
            ON CONFLICT(target_service_id,target_endpoint_id,target_session_id) DO UPDATE SET
                working_directory=excluded.working_directory,
                requested_policy_json=excluded.requested_policy_json,
                approver_json=excluded.approver_json,
                updated_at_ms=excluded.updated_at_ms",
            target_service_id,
            target_endpoint_id,
            target_session_id,
            working_directory,
            requested_policy_json,
            created_by_json,
            approver_json,
            record.updated_at_ms,
        )
        .execute(&mut self.connection)
        .await?;
        Ok(())
    }

    pub async fn session_record(
        &mut self,
        target: &SessionRef,
    ) -> Result<Option<ProviderSessionRecord>, ProviderOperationStoreError> {
        let target_service_id = String::from(target.endpoint.service_id.clone());
        let target_endpoint_id = String::from(target.endpoint.endpoint_id.clone());
        let target_session_id = String::from(target.session_id.clone());
        let row = sqlx::query!(
            "SELECT working_directory,requested_policy_json,created_by_json,approver_json,updated_at_ms
             FROM provider_session_records
             WHERE target_service_id=? AND target_endpoint_id=? AND target_session_id=?",
            target_service_id,
            target_endpoint_id,
            target_session_id,
        )
        .fetch_optional(&mut self.connection)
        .await?;
        row.map(|row| {
            let working_directory = ProviderWorkingDirectory::try_from(row.working_directory)
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
            let requested_policy = serde_json::from_str(&row.requested_policy_json)
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
            let created_by = serde_json::from_str(&row.created_by_json)
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
            let approver = serde_json::from_str(&row.approver_json)
                .map_err(|_| ProviderOperationStoreError::InvalidRecord)?;
            let record = ProviderSessionRecord {
                target: target.clone(),
                working_directory,
                requested_policy,
                created_by,
                approver,
                updated_at_ms: row.updated_at_ms,
            };
            record.validate()?;
            Ok(record)
        })
        .transpose()
    }
}
