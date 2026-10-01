//! Durable, generation-scoped credential renewal transitions.

use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use sqlx::Row;

use crate::account::AccountStatus;
use crate::credential_maintenance::ClaimPurpose;
use crate::credential_maintenance::CredentialFailureClass;
use crate::credential_maintenance::CredentialMaintenanceRecord;
use crate::credential_maintenance::CredentialMaintenanceState;
use crate::credential_maintenance::CredentialRefreshClaimDisposition;
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
                claimed_successor_generation, claim_purpose,
                claim_started_unix_seconds, claim_prior_state, consecutive_failures
             )
             SELECT account_id, ?2, 'retrying', 'local_persistence', NULL, ?3,
                    NULL, NULL, NULL, NULL, 1
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
                claim_purpose = NULL, claim_started_unix_seconds = NULL,
                claim_prior_state = NULL,
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
                    claimed_successor_generation, claim_purpose,
                    claim_started_unix_seconds, claim_prior_state, consecutive_failures
               FROM credential_maintenance
               JOIN accounts USING (account_id)
              WHERE credential_maintenance.account_id = ?1",
        )
        .bind(account_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;
        row.map(|row| decode_maintenance_row(account_id, &row))
            .transpose()
    }

    /// Claims a successor generation in the existing maintenance row.
    pub async fn claim_credential_refresh(
        &self,
        account_id: &AccountId,
        provider: Provider,
        purpose: ClaimPurpose,
        current_generation: u64,
        successor_generation: u64,
        claim_started_unix_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        if successor_generation <= current_generation {
            return Err(corrupt_maintenance(
                account_id,
                "claimed_successor_generation",
            ));
        }
        let login_claim = purpose == ClaimPurpose::Login;
        let updated = sqlx::query!(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, claim_purpose,
                claim_started_unix_seconds, claim_prior_state, consecutive_failures
             )
             SELECT account_id, ?2, 'in_progress', NULL, NULL, NULL,
                    ?3, ?6, ?7, NULL, 0
               FROM accounts
              WHERE account_id = ?1 AND provider = ?4
                AND (
                    (?5 = 0 AND status = 'enabled'
                        AND active_credential_generation = ?2)
                    OR (?5 = 1 AND status IN ('enabled', 'disabled')
                        AND (active_credential_generation = ?2
                            OR (?2 = 0 AND active_credential_generation IS NULL)))
                )
             ON CONFLICT(account_id) DO UPDATE SET
                credential_generation = excluded.credential_generation,
                state = 'in_progress',
                failure_class = CASE
                    WHEN ?5 = 1 AND credential_maintenance.credential_generation = excluded.credential_generation
                         AND credential_maintenance.state = 'in_progress'
                    THEN 'provider_outcome_ambiguous'
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.failure_class END,
                last_success_unix_seconds = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.last_success_unix_seconds END,
                next_attempt_unix_seconds = CASE
                    WHEN ?5 = 1 AND credential_maintenance.credential_generation = excluded.credential_generation
                         AND credential_maintenance.state != 'in_progress'
                    THEN credential_maintenance.next_attempt_unix_seconds ELSE NULL END,
                claimed_successor_generation = excluded.claimed_successor_generation,
                claim_purpose = excluded.claim_purpose,
                claim_started_unix_seconds = excluded.claim_started_unix_seconds,
                claim_prior_state = CASE
                    WHEN ?5 = 1 AND credential_maintenance.credential_generation = excluded.credential_generation
                    THEN CASE WHEN credential_maintenance.state = 'in_progress'
                              THEN 'reauth_required' ELSE credential_maintenance.state END
                    ELSE NULL END,
                consecutive_failures = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN 0 ELSE credential_maintenance.consecutive_failures END
              WHERE credential_maintenance.credential_generation < excluded.credential_generation
                 OR (credential_maintenance.credential_generation = excluded.credential_generation
                     AND credential_maintenance.state != 'in_progress')",
            account_id.as_str(),
            u64_to_i64(current_generation)?,
            u64_to_i64(successor_generation)?,
            provider.as_str(),
            login_claim,
            purpose.as_str(),
            u64_to_i64(claim_started_unix_seconds)?,
        )
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Restores or removes a login claim that exceeded its owner login timeout.
    pub async fn release_stale_login_credential_claim(
        &self,
        account_id: &AccountId,
        provider: Provider,
        now_unix_seconds: u64,
        login_timeout_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        let stale_before_unix_seconds =
            u64_to_i64(now_unix_seconds.saturating_sub(login_timeout_seconds))?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let deleted = sqlx::query!(
            "DELETE FROM credential_maintenance
              WHERE account_id = ?1 AND state = 'in_progress'
                AND claim_purpose = 'login' AND claim_prior_state IS NULL
                AND claim_started_unix_seconds < ?2
                AND EXISTS (
                    SELECT 1 FROM accounts
                     WHERE accounts.account_id = credential_maintenance.account_id
                       AND accounts.provider = ?3
                )",
            account_id.as_str(),
            stale_before_unix_seconds,
            provider.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
        let restored = sqlx::query!(
            "UPDATE credential_maintenance
                SET state = claim_prior_state,
                    claimed_successor_generation = NULL,
                    claim_purpose = NULL,
                    claim_started_unix_seconds = NULL,
                    claim_prior_state = NULL
              WHERE account_id = ?1 AND state = 'in_progress'
                AND claim_purpose = 'login' AND claim_prior_state IS NOT NULL
                AND claim_started_unix_seconds < ?2
                AND EXISTS (
                    SELECT 1 FROM accounts
                     WHERE accounts.account_id = credential_maintenance.account_id
                       AND accounts.provider = ?3
                )",
            account_id.as_str(),
            stale_before_unix_seconds,
            provider.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
        transaction.commit().await.map_err(sqlx_error)?;
        Ok(deleted + restored != 0)
    }

    /// Releases a login claim after the caller acquires its per-account file lock.
    ///
    /// The lock proves the previous login owner has exited, so an age delay would only
    /// waste an authorization code. The returned claim is removed or restored atomically.
    pub async fn release_in_progress_login_credential_claim(
        &self,
        account_id: &AccountId,
        provider: Provider,
        current_generation: u64,
        successor_generation: u64,
    ) -> Result<bool, StateStoreError> {
        let current_generation = u64_to_i64(current_generation)?;
        let successor_generation = u64_to_i64(successor_generation)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let deleted = sqlx::query!(
            "DELETE FROM credential_maintenance
              WHERE account_id = ?1 AND credential_generation = ?2
                AND claimed_successor_generation = ?3 AND state = 'in_progress'
                AND claim_purpose = 'login' AND claim_prior_state IS NULL
                AND EXISTS (
                    SELECT 1 FROM accounts
                     WHERE accounts.account_id = credential_maintenance.account_id
                       AND accounts.provider = ?4
                )",
            account_id.as_str(),
            current_generation,
            successor_generation,
            provider.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
        let restored = sqlx::query!(
            "UPDATE credential_maintenance
                SET state = claim_prior_state,
                    claimed_successor_generation = NULL,
                    claim_purpose = NULL,
                    claim_started_unix_seconds = NULL,
                    claim_prior_state = NULL
              WHERE account_id = ?1 AND credential_generation = ?2
                AND claimed_successor_generation = ?3 AND state = 'in_progress'
                AND claim_purpose = 'login' AND claim_prior_state IS NOT NULL
                AND EXISTS (
                    SELECT 1 FROM accounts
                     WHERE accounts.account_id = credential_maintenance.account_id
                       AND accounts.provider = ?4
                )",
            account_id.as_str(),
            current_generation,
            successor_generation,
            provider.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?
        .rows_affected();
        transaction.commit().await.map_err(sqlx_error)?;
        Ok(deleted + restored != 0)
    }

    /// Restores the prior maintenance row after a login's staged credential write fails.
    pub async fn restore_credential_maintenance_after_login_write_failure(
        &self,
        account_id: &AccountId,
        provider: Provider,
        current_generation: u64,
        successor_generation: u64,
        previous_record: Option<&CredentialMaintenanceRecord>,
    ) -> Result<bool, StateStoreError> {
        let current_generation = u64_to_i64(current_generation)?;
        let successor_generation = u64_to_i64(successor_generation)?;
        match previous_record {
            Some(record) => {
                let updated = sqlx::query(
                    "UPDATE credential_maintenance
                        SET credential_generation = ?4, state = ?5, failure_class = ?6,
                            last_success_unix_seconds = ?7, next_attempt_unix_seconds = ?8,
                            claimed_successor_generation = ?9, consecutive_failures = ?10,
                            claim_purpose = NULL, claim_started_unix_seconds = NULL,
                            claim_prior_state = NULL
                      WHERE account_id = ?1 AND credential_generation = ?2
                        AND claimed_successor_generation = ?3 AND state = 'in_progress'
                        AND EXISTS (
                            SELECT 1 FROM accounts
                             WHERE accounts.account_id = credential_maintenance.account_id
                               AND accounts.provider = ?11
                        )",
                )
                .bind(account_id.as_str())
                .bind(current_generation)
                .bind(successor_generation)
                .bind(u64_to_i64(record.credential_generation)?)
                .bind(record.state.as_str())
                .bind(record.failure_class.map(CredentialFailureClass::as_str))
                .bind(
                    record
                        .last_success_unix_seconds
                        .map(u64_to_i64)
                        .transpose()?,
                )
                .bind(
                    record
                        .next_attempt_unix_seconds
                        .map(u64_to_i64)
                        .transpose()?,
                )
                .bind(
                    record
                        .claimed_successor_generation
                        .map(u64_to_i64)
                        .transpose()?,
                )
                .bind(i64::from(record.consecutive_failures))
                .bind(provider.as_str())
                .execute(&self.pool)
                .await
                .map_err(sqlx_error)?;
                Ok(updated.rows_affected() == 1)
            }
            None => {
                let deleted = sqlx::query(
                    "DELETE FROM credential_maintenance
                      WHERE account_id = ?1 AND credential_generation = ?2
                        AND claimed_successor_generation = ?3 AND state = 'in_progress'
                        AND EXISTS (
                            SELECT 1 FROM accounts
                             WHERE accounts.account_id = credential_maintenance.account_id
                               AND accounts.provider = ?4
                        )",
                )
                .bind(account_id.as_str())
                .bind(current_generation)
                .bind(successor_generation)
                .bind(provider.as_str())
                .execute(&self.pool)
                .await
                .map_err(sqlx_error)?;
                Ok(deleted.rows_affected() == 1)
            }
        }
    }

    /// Disposes a claim after a confirmed unspent or terminal provider result.
    pub async fn finish_credential_refresh_claim(
        &self,
        account_id: &AccountId,
        provider: Provider,
        current_generation: u64,
        successor_generation: u64,
        disposition: CredentialRefreshClaimDisposition,
    ) -> Result<bool, StateStoreError> {
        let state = disposition.state();
        let failure_class = disposition.failure_class();
        let next_attempt_unix_seconds = disposition.next_attempt_unix_seconds();
        let updated = sqlx::query(
            "UPDATE credential_maintenance
                SET state = ?4, failure_class = ?5,
                    next_attempt_unix_seconds = ?6,
                    claimed_successor_generation = NULL,
                    claim_purpose = NULL, claim_started_unix_seconds = NULL,
                    claim_prior_state = NULL,
                    consecutive_failures = consecutive_failures + CASE WHEN ?4 = 'retrying' THEN 1 ELSE 0 END
              WHERE account_id = ?1 AND credential_generation = ?2
                AND claimed_successor_generation = ?3 AND state = 'in_progress'
                AND EXISTS (
                    SELECT 1 FROM accounts
                     WHERE accounts.account_id = credential_maintenance.account_id
                       AND accounts.provider = ?7
                )",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(u64_to_i64(successor_generation)?)
        .bind(state.as_str())
        .bind(failure_class.as_str())
        .bind(next_attempt_unix_seconds.map(u64_to_i64).transpose()?)
        .bind(provider.as_str())
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Marks a still-active provider-rejected generation as requiring reauthentication.
    ///
    /// A newer active generation or an in-progress successor claim wins over this observation.
    pub async fn mark_generation_reauth_required(
        &self,
        account_id: &AccountId,
        rejected_generation: u64,
    ) -> Result<bool, StateStoreError> {
        let updated = sqlx::query!(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, claim_purpose,
                claim_started_unix_seconds, claim_prior_state, consecutive_failures
             )
             SELECT account_id, ?2, 'reauth_required', 'provider_rejected',
                    NULL, NULL, NULL, NULL, NULL, NULL, 0
               FROM accounts
              WHERE account_id = ?1 AND active_credential_generation = ?2
             ON CONFLICT(account_id) DO UPDATE SET
                credential_generation = excluded.credential_generation,
                state = 'reauth_required',
                failure_class = 'provider_rejected',
                last_success_unix_seconds = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.last_success_unix_seconds END,
                next_attempt_unix_seconds = NULL,
                claimed_successor_generation = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.claimed_successor_generation END,
                claim_purpose = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.claim_purpose END,
                claim_started_unix_seconds = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.claim_started_unix_seconds END,
                claim_prior_state = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN NULL ELSE credential_maintenance.claim_prior_state END,
                consecutive_failures = CASE
                    WHEN credential_maintenance.credential_generation < excluded.credential_generation
                    THEN 0 ELSE credential_maintenance.consecutive_failures END
              WHERE credential_maintenance.credential_generation <= excluded.credential_generation
                AND credential_maintenance.state != 'in_progress'
                AND credential_maintenance.claimed_successor_generation IS NULL",
            account_id.as_str(),
            u64_to_i64(rejected_generation)?,
        )
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        Ok(updated.rows_affected() == 1)
    }

    /// Activates only the secret slot reserved by this claim and clears the claim atomically.
    pub async fn activate_claimed_credential_generation(
        &self,
        account_id: &AccountId,
        provider: Provider,
        purpose: ClaimPurpose,
        current_generation: u64,
        successor_generation: u64,
        now_unix_seconds: u64,
    ) -> Result<bool, StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let claim_consumed = match purpose {
            ClaimPurpose::Refresh => {
                sqlx::query!(
                    "UPDATE credential_maintenance
                        SET credential_generation = ?3, state = 'healthy', failure_class = NULL,
                            last_success_unix_seconds = ?4, next_attempt_unix_seconds = NULL,
                            claimed_successor_generation = NULL, consecutive_failures = 0,
                            claim_purpose = NULL, claim_started_unix_seconds = NULL,
                            claim_prior_state = NULL
                      WHERE account_id = ?1 AND credential_generation = ?2
                        AND claimed_successor_generation = ?3 AND state = 'in_progress'
                        AND EXISTS (
                            SELECT 1 FROM accounts
                             WHERE accounts.account_id = credential_maintenance.account_id
                               AND accounts.provider = ?5
                        )",
                    account_id.as_str(),
                    u64_to_i64(current_generation)?,
                    u64_to_i64(successor_generation)?,
                    u64_to_i64(now_unix_seconds)?,
                    provider.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?
                .rows_affected()
                    == 1
            }
            ClaimPurpose::Login => {
                sqlx::query!(
                    "DELETE FROM credential_maintenance
                      WHERE account_id = ?1 AND credential_generation = ?2
                        AND claimed_successor_generation = ?3 AND state = 'in_progress'
                        AND EXISTS (
                            SELECT 1 FROM accounts
                             WHERE accounts.account_id = credential_maintenance.account_id
                               AND accounts.provider = ?4
                        )",
                    account_id.as_str(),
                    u64_to_i64(current_generation)?,
                    u64_to_i64(successor_generation)?,
                    provider.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?
                .rows_affected()
                    == 1
            }
        };
        if !claim_consumed {
            transaction.rollback().await.map_err(sqlx_error)?;
            return Ok(false);
        }
        let account_activated = match purpose {
            ClaimPurpose::Refresh => {
                sqlx::query!(
                    "UPDATE accounts SET active_credential_generation = ?3
                      WHERE account_id = ?1 AND active_credential_generation = ?2
                        AND status = ?4 AND provider = ?5",
                    account_id.as_str(),
                    u64_to_i64(current_generation)?,
                    u64_to_i64(successor_generation)?,
                    AccountStatus::Enabled.as_str(),
                    provider.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?
                .rows_affected()
                    == 1
            }
            ClaimPurpose::Login => {
                sqlx::query!(
                    "UPDATE accounts SET status = ?5, active_credential_generation = ?3
                      WHERE account_id = ?1 AND provider = ?4
                        AND status IN ('enabled', 'disabled')
                        AND (active_credential_generation = ?2
                            OR (?2 = 0 AND active_credential_generation IS NULL))",
                    account_id.as_str(),
                    u64_to_i64(current_generation)?,
                    u64_to_i64(successor_generation)?,
                    provider.as_str(),
                    AccountStatus::Enabled.as_str(),
                )
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?
                .rows_affected()
                    == 1
            }
        };
        if !account_activated {
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
        provider: Provider,
        current_generation: u64,
    ) -> Result<bool, StateStoreError> {
        let updated = sqlx::query(
            "INSERT INTO credential_maintenance (
                account_id, credential_generation, state, failure_class,
                last_success_unix_seconds, next_attempt_unix_seconds,
                claimed_successor_generation, claim_purpose,
                claim_started_unix_seconds, claim_prior_state, consecutive_failures
             )
             SELECT account_id, ?2, 'unrefreshable', NULL, NULL, NULL,
                    NULL, NULL, NULL, NULL, 0
               FROM accounts
              WHERE account_id = ?1 AND active_credential_generation = ?2 AND provider = ?3
             ON CONFLICT(account_id) DO UPDATE SET
                state = 'unrefreshable', failure_class = NULL,
                next_attempt_unix_seconds = NULL,
                claimed_successor_generation = NULL,
                claim_purpose = NULL, claim_started_unix_seconds = NULL,
                claim_prior_state = NULL
              WHERE credential_maintenance.credential_generation = excluded.credential_generation
                AND credential_maintenance.state != 'in_progress'",
        )
        .bind(account_id.as_str())
        .bind(u64_to_i64(current_generation)?)
        .bind(provider.as_str())
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
    let claim_purpose_value: Option<String> = row.try_get("claim_purpose").map_err(sqlx_error)?;
    let claim_purpose = claim_purpose_value
        .as_deref()
        .map(|value| {
            ClaimPurpose::parse(value)
                .ok_or_else(|| corrupt_maintenance(account_id, "claim_purpose"))
        })
        .transpose()?;
    let claim_started_unix_seconds = field_value("claim_started_unix_seconds")?;
    let claim_prior_state_value: Option<String> =
        row.try_get("claim_prior_state").map_err(sqlx_error)?;
    let claim_prior_state = claim_prior_state_value
        .as_deref()
        .map(|value| {
            CredentialMaintenanceState::parse(value)
                .ok_or_else(|| corrupt_maintenance(account_id, "claim_prior_state"))
        })
        .transpose()?;
    let next_attempt_unix_seconds = field_value("next_attempt_unix_seconds")?;
    let consecutive_failures = field_value("consecutive_failures")?
        .and_then(|count| u32::try_from(count).ok())
        .ok_or_else(|| corrupt_maintenance(account_id, "consecutive_failures"))?;
    let claim_is_active = state == CredentialMaintenanceState::InProgress;
    if claim_is_active != claimed_successor_generation.is_some()
        || claim_is_active != claim_purpose.is_some()
        || claim_is_active != claim_started_unix_seconds.is_some()
        || (!claim_is_active
            && (claim_prior_state.is_some()
                || claim_purpose.is_some()
                || claim_started_unix_seconds.is_some()))
        || (claim_prior_state.is_some() && claim_purpose != Some(ClaimPurpose::Login))
        || claim_prior_state == Some(CredentialMaintenanceState::InProgress)
        || claimed_successor_generation.is_some_and(|successor| successor <= credential_generation)
        || ((state == CredentialMaintenanceState::Retrying
            || (state == CredentialMaintenanceState::InProgress
                && claim_purpose == Some(ClaimPurpose::Login)
                && claim_prior_state == Some(CredentialMaintenanceState::Retrying)))
            != next_attempt_unix_seconds.is_some())
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
        claim_purpose,
        claim_started_unix_seconds,
        claim_prior_state,
        consecutive_failures,
    })
}

fn corrupt_maintenance(account_id: &AccountId, field: &'static str) -> StateStoreError {
    StateStoreError::CorruptAccount {
        account_id: account_id.as_str().to_owned(),
        field,
    }
}
