//! Durable, generation-scoped credential renewal transitions.

use codex_router_core::ids::AccountId;
use sqlx::Row;

use crate::account::AccountStatus;
use crate::credential_maintenance::CredentialFailureClass;
use crate::credential_maintenance::CredentialMaintenanceRecord;
use crate::credential_maintenance::CredentialMaintenanceState;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;
use crate::sqlite::invalidate_credential_mutation_quota_async;
use crate::sqlite::sqlx_error;
use crate::sqlite::u64_to_i64;

impl AsyncSqliteStateStore {
    /// Records a retryable local failure before any provider use for this generation.
    pub async fn record_pre_provider_local_failure(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        now_unix_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        let previous = self.load_credential_maintenance(account_id).await?;
        let failures = previous
            .as_ref()
            .filter(|record| record.credential_generation == current_generation)
            .map_or(0, |record| record.consecutive_failures);
        let delay = 60_u64.saturating_mul(1_u64 << failures.min(5)).min(30 * 60);
        let retry_at = now_unix_seconds.saturating_add(delay);
        let updated = sqlx::query(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, consecutive_failures
             )
             SELECT account_id, ?2, 'retrying', 'local_persistence', NULL, ?3, NULL, 1
               FROM accounts
              WHERE account_id = ?1 AND status = ?4 AND active_credential_generation = ?2
             ON CONFLICT(account_id) DO UPDATE SET
                credential_generation = excluded.credential_generation,
                state = 'retrying', failure_class = 'local_persistence',
                last_success_unix_seconds = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.last_success_unix_seconds END,
                next_attempt_unix_seconds = excluded.next_attempt_unix_seconds,
                claimed_successor_generation = NULL,
                consecutive_failures = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN 1 ELSE credential_maintenance.consecutive_failures + 1 END
              WHERE credential_maintenance.credential_generation < excluded.credential_generation
                 OR (credential_maintenance.credential_generation = excluded.credential_generation
                     AND credential_maintenance.state NOT IN ('in_progress', 'reauth_required', 'unrefreshable')
                     AND (credential_maintenance.next_attempt_unix_seconds IS NULL
                          OR credential_maintenance.next_attempt_unix_seconds <= ?5))",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(retry_at)?)
        .bind(AccountStatus::Enabled.as_str())
        .bind(u64_to_i64(now_unix_seconds)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Loads one non-secret maintenance result, rejecting malformed stored values.
    pub async fn load_credential_maintenance(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<CredentialMaintenanceRecord>, StateStoreError> {
        if self.read_only {
            let table_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'credential_maintenance')",
            )
            .fetch_one(&self.pool)
            .await
            .map_err(sqlx_error)?;
            if !table_exists {
                return Ok(None);
            }
        }
        let row = sqlx::query(
            "SELECT credential_generation, state, failure_class,
                    last_success_unix_seconds, next_attempt_unix_seconds,
                    claimed_successor_generation, consecutive_failures
               FROM credential_maintenance WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;
        row.map(|row| decode_maintenance_row(account_id, &row))
            .transpose()
    }

    /// Claims the current generation before provider egress.
    pub async fn claim_credential_refresh(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        successor_generation: u64,
    ) -> Result<bool, StateStoreError> {
        if successor_generation <= current_generation {
            return Err(corrupt_maintenance(
                account_id,
                "claimed_successor_generation",
            ));
        }
        let updated = sqlx::query(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, consecutive_failures
             )
             SELECT account_id, ?2, 'in_progress', NULL, NULL, NULL, ?3, 0
               FROM accounts
              WHERE account_id = ?1 AND status = ?4
                AND active_credential_generation = ?2
             ON CONFLICT(account_id) DO UPDATE SET
                credential_generation = excluded.credential_generation,
                state = 'in_progress', failure_class = NULL,
                last_success_unix_seconds = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.last_success_unix_seconds END,
                next_attempt_unix_seconds = NULL,
                claimed_successor_generation = excluded.claimed_successor_generation,
                consecutive_failures = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN 0 ELSE credential_maintenance.consecutive_failures END
              WHERE credential_maintenance.credential_generation < excluded.credential_generation
                 OR (credential_maintenance.credential_generation = excluded.credential_generation
                     AND credential_maintenance.state != 'in_progress')",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(successor_generation)?)
        .bind(AccountStatus::Enabled.as_str())
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Disposes a claim after a confirmed unspent or terminal provider result.
    pub async fn finish_credential_refresh_claim(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        successor_generation: u64,
        state: CredentialMaintenanceState,
        failure_class: CredentialFailureClass,
        next_attempt_unix_seconds: Option<u64>,
    ) -> Result<bool, StateStoreError> {
        if matches!(
            state,
            CredentialMaintenanceState::InProgress | CredentialMaintenanceState::Healthy
        ) || (state == CredentialMaintenanceState::Retrying)
            != next_attempt_unix_seconds.is_some()
        {
            return Err(corrupt_maintenance(account_id, "state"));
        }
        let updated = sqlx::query(
            "UPDATE credential_maintenance
                SET state = ?4, failure_class = ?5,
                    next_attempt_unix_seconds = ?6,
                    claimed_successor_generation = NULL,
                    consecutive_failures = consecutive_failures + CASE WHEN ?4 = 'retrying' THEN 1 ELSE 0 END
              WHERE account_id = ?1 AND credential_generation = ?2
                AND claimed_successor_generation = ?3 AND state = 'in_progress'",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(successor_generation)?)
        .bind(state.as_str())
        .bind(failure_class.as_str())
        .bind(next_attempt_unix_seconds.map(u64_to_i64).transpose()?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Activates only the secret slot reserved by this claim and clears the claim atomically.
    pub async fn activate_claimed_credential_generation(
        &self,
        account_id: &AccountId,
        current_generation: u64,
        successor_generation: u64,
        now_unix_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let claim_update = sqlx::query(
            "UPDATE credential_maintenance
                SET credential_generation = ?3, state = 'healthy', failure_class = NULL,
                    last_success_unix_seconds = ?4, next_attempt_unix_seconds = NULL,
                    claimed_successor_generation = NULL, consecutive_failures = 0
              WHERE account_id = ?1 AND credential_generation = ?2
                AND claimed_successor_generation = ?3 AND state = 'in_progress'",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(successor_generation)?)
        .bind(u64_to_i64(now_unix_seconds)?)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        if claim_update.rows_affected() != 1 {
            transaction.rollback().await.map_err(sqlx_error)?;
            return Ok(false);
        }
        let account_update = sqlx::query(
            "UPDATE accounts SET active_credential_generation = ?3
              WHERE account_id = ?1 AND active_credential_generation = ?2
                AND status = ?4",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(successor_generation)?)
        .bind(AccountStatus::Enabled.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        if account_update.rows_affected() != 1 {
            transaction.rollback().await.map_err(sqlx_error)?;
            return Ok(false);
        }
        invalidate_credential_mutation_quota_async(&mut transaction, account_id).await?;
        transaction.commit().await.map_err(sqlx_error)?;
        Ok(true)
    }

    /// Records missing renewable material for the current generation.
    pub async fn mark_credential_unrefreshable(
        &self,
        account_id: &AccountId,
        current_generation: u64,
    ) -> Result<bool, StateStoreError> {
        let updated = sqlx::query(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, consecutive_failures
             )
             SELECT account_id, ?2, 'unrefreshable', NULL, NULL, NULL, NULL, 0
               FROM accounts
              WHERE account_id = ?1 AND active_credential_generation = ?2
             ON CONFLICT(account_id) DO UPDATE SET
                state = 'unrefreshable', failure_class = NULL,
                next_attempt_unix_seconds = NULL
              WHERE credential_maintenance.credential_generation = excluded.credential_generation
                AND credential_maintenance.state != 'in_progress'",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }
}

fn decode_maintenance_row(
    account_id: &AccountId,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<CredentialMaintenanceRecord, StateStoreError> {
    let field_value = |field: &'static str| -> Result<Option<u64>, StateStoreError> {
        row.try_get::<Option<i64>, _>(field)
            .map_err(sqlx_error)?
            .map(|value| u64::try_from(value).map_err(|_| corrupt_maintenance(account_id, field)))
            .transpose()
    };
    let credential_generation = field_value("credential_generation")?
        .ok_or_else(|| corrupt_maintenance(account_id, "credential_generation"))?;
    let state_value: String = row.try_get("state").map_err(sqlx_error)?;
    let state = CredentialMaintenanceState::parse(&state_value)
        .ok_or_else(|| corrupt_maintenance(account_id, "state"))?;
    let failure_value: Option<String> = row.try_get("failure_class").map_err(sqlx_error)?;
    let failure_class = failure_value
        .as_deref()
        .map(|value| {
            CredentialFailureClass::parse(value)
                .ok_or_else(|| corrupt_maintenance(account_id, "failure_class"))
        })
        .transpose()?;
    let claimed_successor_generation = field_value("claimed_successor_generation")?;
    let next_attempt_unix_seconds = field_value("next_attempt_unix_seconds")?;
    let consecutive_failures = field_value("consecutive_failures")?
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(|| corrupt_maintenance(account_id, "consecutive_failures"))?;
    if (state == CredentialMaintenanceState::InProgress) != claimed_successor_generation.is_some()
        || claimed_successor_generation.is_some_and(|successor| successor <= credential_generation)
        || (state == CredentialMaintenanceState::Retrying) != next_attempt_unix_seconds.is_some()
    {
        return Err(corrupt_maintenance(account_id, "state"));
    }
    Ok(CredentialMaintenanceRecord {
        credential_generation,
        state,
        failure_class,
        last_success_unix_seconds: field_value("last_success_unix_seconds")?,
        next_attempt_unix_seconds,
        claimed_successor_generation,
        consecutive_failures,
    })
}

fn corrupt_maintenance(account_id: &AccountId, field: &'static str) -> StateStoreError {
    StateStoreError::CorruptAccount {
        account_id: account_id.as_str().to_owned(),
        field,
    }
}
