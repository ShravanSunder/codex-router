//! Ordered refresh attempt allocation and atomic Responses observation commits.

use codex_router_core::credit_usage::CreditAvailability;
use codex_router_core::credit_usage::CreditBalance;
use codex_router_core::credit_usage::CreditProviderLimitReason;
use codex_router_core::ids::AccountId;

use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::QuotaHistoryRefreshOutcome;
use crate::quota_snapshot::QuotaRefreshErrorClass;
use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;

use super::CreditRefreshAttempt;
use super::ResponsesRefreshSuccessCommit;
use super::corrupt_credit_account;
use super::stored_nonnegative_u64;

impl AsyncSqliteStateStore {
    /// Allocates a cross-process monotonic attempt before one Responses provider read.
    pub async fn begin_credit_refresh_attempt(
        &self,
        account_id: &AccountId,
        credential_generation: u64,
    ) -> Result<CreditRefreshAttempt, StateStoreError> {
        let credential_generation = crate::sqlite::u64_to_i64(credential_generation)?;
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        let allocated = sqlx::query!(
            "INSERT INTO account_credit_observations (
               account_id, credential_generation, latest_started_attempt,
               committed_attempt, observed_unix_seconds, stale_after_unix_seconds,
               availability, balance, spend_control_state, provider_limit_reason
             )
             SELECT ?1, ?2, 1, NULL, NULL, NULL, 'unknown', NULL, 'unreported', NULL
              WHERE EXISTS (
                SELECT 1 FROM accounts
                 WHERE account_id = ?1 AND active_credential_generation = ?2
              )
             ON CONFLICT(account_id) DO UPDATE SET
               credential_generation = excluded.credential_generation,
               latest_started_attempt = account_credit_observations.latest_started_attempt + 1,
               committed_attempt = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.committed_attempt
                 ELSE NULL
               END,
               observed_unix_seconds = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.observed_unix_seconds
                 ELSE NULL
               END,
               stale_after_unix_seconds = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.stale_after_unix_seconds
                 ELSE NULL
               END,
               availability = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.availability
                 ELSE 'unknown'
               END,
               balance = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.balance
                 ELSE NULL
               END,
               spend_control_state = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.spend_control_state
                 ELSE 'unreported'
               END,
               provider_limit_reason = CASE
                 WHEN account_credit_observations.credential_generation = excluded.credential_generation
                   THEN account_credit_observations.provider_limit_reason
                 ELSE NULL
               END
             WHERE account_credit_observations.latest_started_attempt > 0
               AND account_credit_observations.latest_started_attempt < 9223372036854775807
             RETURNING latest_started_attempt",
            account_id.as_str(),
            credential_generation,
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;

        let Some(allocated) = allocated else {
            let current = sqlx::query!(
                "SELECT accounts.active_credential_generation AS active_credential_generation,
                        account_credit_observations.latest_started_attempt AS \"latest_started_attempt?\"
                   FROM accounts
                   LEFT JOIN account_credit_observations
                     ON account_credit_observations.account_id = accounts.account_id
                  WHERE accounts.account_id = ?1",
                account_id.as_str(),
            )
            .fetch_optional(&mut *transaction)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
            let error = match current {
                Some(row) if row.active_credential_generation == Some(credential_generation) => {
                    match row.latest_started_attempt {
                        Some(latest) if latest <= 0 => {
                            corrupt_credit_account(account_id, "latest_started_credit_attempt")
                        }
                        Some(i64::MAX) => StateStoreError::CreditRefreshAttemptSequenceOverflow,
                        _ => StateStoreError::Sqlite {
                            message: "credit refresh attempt could not be allocated".to_owned(),
                        },
                    }
                }
                _ => StateStoreError::AccountConcurrentModification {
                    account_id: account_id.as_str().to_owned(),
                },
            };
            transaction
                .rollback()
                .await
                .map_err(crate::sqlite::sqlx_error)?;
            return Err(error);
        };

        let sequence = stored_nonnegative_u64(
            allocated.latest_started_attempt,
            account_id,
            "latest_started_credit_attempt",
        )?;
        if sequence == 0 {
            transaction
                .rollback()
                .await
                .map_err(crate::sqlite::sqlx_error)?;
            return Err(corrupt_credit_account(
                account_id,
                "latest_started_credit_attempt",
            ));
        }
        transaction
            .commit()
            .await
            .map_err(crate::sqlite::sqlx_error)?;

        Ok(CreditRefreshAttempt {
            account_id: account_id.clone(),
            credential_generation: u64::try_from(credential_generation)
                .map_err(|_| corrupt_credit_account(account_id, "active_credential_generation"))?,
            sequence,
        })
    }

    /// Atomically stores quota windows, refresh status and credit facts for the latest attempt.
    ///
    /// Returns false when a newer attempt or credential generation superseded this response.
    pub async fn record_responses_refresh_success(
        &self,
        commit: ResponsesRefreshSuccessCommit<'_>,
    ) -> Result<bool, StateStoreError> {
        let ResponsesRefreshSuccessCommit {
            attempt,
            selector_windows: windows,
            observed_unix_seconds,
            stale_after_unix_seconds,
            provider_observation,
            history_observations,
            snapshot,
        } = commit;
        if windows.is_empty()
            || history_observations.len() != windows.len()
            || stale_after_unix_seconds < observed_unix_seconds
            || snapshot.account_id() != &attempt.account_id
            || snapshot.route_band() != "responses"
            || snapshot.observed_unix_seconds() != observed_unix_seconds
            || windows.iter().any(|window| {
                window.account_id() != &attempt.account_id
                    || window.route_band() != "responses"
                    || window.observed_unix_seconds() != observed_unix_seconds
            })
            || history_observations.iter().any(|observation| {
                observation.account_id() != &attempt.account_id
                    || observation.route_band() != "responses"
                    || observation.observed_unix_seconds() != observed_unix_seconds
                    || observation.refresh_outcome() != QuotaHistoryRefreshOutcome::Success
                    || !windows.iter().any(|window| {
                        window.limit_window_seconds() == observation.limit_window_seconds()
                            && window.remaining_headroom() == observation.remaining_headroom()
                            && window.reset_unix_seconds() == observation.reset_unix_seconds()
                            && window.status() == observation.window_status()
                            && window.effective() == observation.effective()
                    })
            })
        {
            return Err(StateStoreError::InvalidCreditRefreshInput {
                field: "responses_snapshot",
            });
        }

        let credential_generation = crate::sqlite::u64_to_i64(attempt.credential_generation)?;
        let sequence = crate::sqlite::u64_to_i64(attempt.sequence)?;
        let observed = crate::sqlite::u64_to_i64(observed_unix_seconds)?;
        let stale_after = crate::sqlite::u64_to_i64(stale_after_unix_seconds)?;
        let availability = provider_observation.availability();
        let balance = match availability {
            CreditAvailability::Available { balance } => {
                balance.as_ref().map(CreditBalance::as_str)
            }
            CreditAvailability::Unknown
            | CreditAvailability::Depleted
            | CreditAvailability::Unlimited => None,
        };
        let spend_control = provider_observation.spend_control().as_str();
        let provider_limit_reason = provider_observation
            .limit_reason()
            .map(CreditProviderLimitReason::as_str);

        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        let updated = sqlx::query!(
            "UPDATE account_credit_observations
                SET committed_attempt = ?1,
                    observed_unix_seconds = ?2,
                    stale_after_unix_seconds = ?3,
                    availability = ?4,
                    balance = ?5,
                    spend_control_state = ?6,
                    provider_limit_reason = ?7
              WHERE account_id = ?8
                AND credential_generation = ?9
                AND latest_started_attempt = ?10
                AND EXISTS (
                  SELECT 1 FROM accounts
                   WHERE accounts.account_id = account_credit_observations.account_id
                     AND accounts.active_credential_generation = ?11
                )",
            sequence,
            observed,
            stale_after,
            availability.as_str(),
            balance,
            spend_control,
            provider_limit_reason,
            attempt.account_id.as_str(),
            credential_generation,
            sequence,
            credential_generation,
        )
        .execute(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
        if updated.rows_affected() != 1 {
            transaction
                .rollback()
                .await
                .map_err(crate::sqlite::sqlx_error)?;
            return Ok(false);
        }

        sqlx::query!(
            "DELETE FROM selector_quota_windows
              WHERE account_id = ?1 AND route_band = 'responses'",
            attempt.account_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
        sqlx::query!(
            "DELETE FROM route_band_account_states
              WHERE account_id = ?1 AND route_band = 'responses'",
            attempt.account_id.as_str(),
        )
        .execute(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
        for window in windows {
            let limit_window_seconds = crate::sqlite::u64_to_i64(window.limit_window_seconds())?;
            let reset_unix_seconds = window
                .reset_unix_seconds()
                .map(crate::sqlite::u64_to_i64)
                .transpose()?;
            let effective = if window.effective() { 1_i64 } else { 0_i64 };
            sqlx::query!(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 ) VALUES (?1, 'responses', ?2, ?3, ?4, ?5, ?6, ?7)",
                attempt.account_id.as_str(),
                limit_window_seconds,
                window.status().as_str(),
                i64::from(window.remaining_headroom()),
                reset_unix_seconds,
                effective,
                observed,
            )
            .execute(&mut *transaction)
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        }
        sqlx::query!(
            "INSERT INTO quota_refresh_status (
               account_id, route_band, last_success_unix_seconds,
               last_attempt_unix_seconds, last_error_class,
               stale_after_unix_seconds
             ) VALUES (?1, 'responses', ?2, ?2, NULL, ?3)
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               last_success_unix_seconds = excluded.last_success_unix_seconds,
               last_attempt_unix_seconds = excluded.last_attempt_unix_seconds,
               last_error_class = NULL,
               stale_after_unix_seconds = excluded.stale_after_unix_seconds",
            attempt.account_id.as_str(),
            observed,
            stale_after,
        )
        .execute(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;

        for observation in history_observations {
            crate::sqlite::insert_quota_history_observation_in_async_transaction(
                &mut transaction,
                observation,
            )
            .await?;
        }
        crate::sqlite::upsert_quota_snapshot_in_async_transaction(&mut transaction, snapshot)
            .await?;

        transaction
            .commit()
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        Ok(true)
    }

    /// Records failure status/history only when this Responses attempt is still latest.
    ///
    /// Cached credit facts keep their original observation time; the pending attempt continues
    /// to suppress their admission through the latest-started/committed sequence mismatch.
    pub async fn record_responses_refresh_failure(
        &self,
        attempt: &CreditRefreshAttempt,
        observed_unix_seconds: u64,
        error_class: QuotaRefreshErrorClass,
        history_observations: &[PersistedQuotaHistoryObservation],
    ) -> Result<bool, StateStoreError> {
        if history_observations.len() != 2
            || history_observations.iter().any(|observation| {
                observation.account_id() != &attempt.account_id
                    || observation.route_band() != "responses"
                    || observation.observed_unix_seconds() != observed_unix_seconds
                    || observation.window_status()
                        != crate::quota_snapshot::SelectorQuotaWindowStatus::Unknown
                    || observation.refresh_outcome()
                        != QuotaHistoryRefreshOutcome::Failure { error_class }
                    || !matches!(observation.limit_window_seconds(), 18_000 | 604_800)
            })
        {
            return Err(StateStoreError::InvalidCreditRefreshInput {
                field: "responses_failure_history",
            });
        }

        let credential_generation = crate::sqlite::u64_to_i64(attempt.credential_generation)?;
        let sequence = crate::sqlite::u64_to_i64(attempt.sequence)?;
        let observed = crate::sqlite::u64_to_i64(observed_unix_seconds)?;
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        let recorded = sqlx::query!(
            "INSERT INTO quota_refresh_status (
               account_id, route_band, last_success_unix_seconds,
               last_attempt_unix_seconds, last_error_class,
               stale_after_unix_seconds
             )
             SELECT ?1, 'responses', NULL, ?2, ?3, ?2
              WHERE EXISTS (
                SELECT 1 FROM account_credit_observations
                 JOIN accounts USING (account_id)
                 WHERE account_credit_observations.account_id = ?1
                   AND account_credit_observations.credential_generation = ?4
                   AND account_credit_observations.latest_started_attempt = ?5
                   AND accounts.active_credential_generation = ?4
              )
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               last_attempt_unix_seconds = excluded.last_attempt_unix_seconds,
               last_error_class = excluded.last_error_class,
               stale_after_unix_seconds =
                 CASE
                   WHEN quota_refresh_status.stale_after_unix_seconds IS NULL
                     THEN excluded.stale_after_unix_seconds
                   WHEN quota_refresh_status.stale_after_unix_seconds < excluded.stale_after_unix_seconds
                     THEN quota_refresh_status.stale_after_unix_seconds
                   ELSE excluded.stale_after_unix_seconds
                 END
             WHERE EXISTS (
               SELECT 1 FROM account_credit_observations
                JOIN accounts USING (account_id)
                WHERE account_credit_observations.account_id = ?1
                  AND account_credit_observations.credential_generation = ?4
                  AND account_credit_observations.latest_started_attempt = ?5
                  AND accounts.active_credential_generation = ?4
             )",
            attempt.account_id.as_str(),
            observed,
            error_class.as_str(),
            credential_generation,
            sequence,
        )
        .execute(&mut *transaction)
        .await
        .map_err(crate::sqlite::sqlx_error)?;
        if recorded.rows_affected() != 1 {
            transaction
                .rollback()
                .await
                .map_err(crate::sqlite::sqlx_error)?;
            return Ok(false);
        }

        for observation in history_observations {
            crate::sqlite::insert_quota_history_observation_in_async_transaction(
                &mut transaction,
                observation,
            )
            .await?;
        }
        transaction
            .commit()
            .await
            .map_err(crate::sqlite::sqlx_error)?;
        Ok(true)
    }
}

/// Clears cached facts when an account's credential generation changes.
pub(crate) async fn invalidate_credit_observation_after_credential_mutation(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: &AccountId,
    credential_generation: u64,
) -> Result<(), StateStoreError> {
    let credential_generation = crate::sqlite::u64_to_i64(credential_generation)?;
    sqlx::query!(
        "UPDATE account_credit_observations
            SET credential_generation = ?2,
                committed_attempt = NULL,
                observed_unix_seconds = NULL,
                stale_after_unix_seconds = NULL,
                availability = 'unknown',
                balance = NULL,
                spend_control_state = 'unreported',
                provider_limit_reason = NULL
          WHERE account_id = ?1",
        account_id.as_str(),
        credential_generation,
    )
    .execute(&mut **transaction)
    .await
    .map_err(crate::sqlite::sqlx_error)?;
    Ok(())
}
