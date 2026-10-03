//! SQLite account store responsibilities.
use super::*;
pub(super) const CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS: &[&str] = &[
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
    "code_review",
];

impl AsyncSqliteStateStore {
    /// Inserts or updates account metadata through the async state pool.
    pub async fn upsert_account(&self, account: &AccountRecord) -> Result<(), StateStoreError> {
        let account_id = account.account_id().as_str();
        let account_label = account.label();
        let account_status = account.status().as_str();
        let provider = account.provider().as_str();
        let active_credential_generation = account
            .active_credential_generation()
            .map(u64_to_i64)
            .transpose()?;
        let result = sqlx::query!(
            "INSERT INTO accounts (
                account_id, label, status, active_credential_generation, provider
             )
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id) DO UPDATE SET
               label = excluded.label,
               status = excluded.status,
               active_credential_generation = excluded.active_credential_generation
             WHERE accounts.provider = excluded.provider",
            account_id,
            account_label,
            account_status,
            active_credential_generation,
            provider,
        )
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;
        if result.rows_affected() == 0 {
            return Err(StateStoreError::AccountProviderImmutable);
        }

        Ok(())
    }

    /// Lists account metadata in deterministic selector order through the async state pool.
    pub async fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        let rows = sqlx::query!(
            "SELECT account_id, label, status, active_credential_generation, provider
               FROM accounts
              ORDER BY account_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut accounts = Vec::new();
        for row in rows {
            accounts.push(parse_account_row(
                row.account_id,
                row.label,
                row.status,
                row.active_credential_generation,
                row.provider,
            )?);
        }

        Ok(accounts)
    }

    /// Bulk-loads every enabled per-account routing policy.
    pub async fn list_account_routing_policies(
        &self,
    ) -> Result<Vec<AccountRoutingPolicy>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, weekly_quota_floor_basis_points
               FROM account_routing_policies
              ORDER BY account_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        rows.into_iter()
            .map(parse_account_routing_policy_row)
            .collect()
    }

    /// Loads account metadata through the async state connection pool.
    pub async fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError> {
        let account_id_value = account_id.as_str();
        let row = sqlx::query!(
            "SELECT account_id, label, status, active_credential_generation, provider
               FROM accounts
              WHERE account_id = ?1",
            account_id_value,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        parse_account_row(
            row.account_id,
            row.label,
            row.status,
            row.active_credential_generation,
            row.provider,
        )
        .map(Some)
    }

    /// Returns the next credential generation through the async state pool.
    pub async fn next_credential_generation(
        &self,
        account_id: &AccountId,
    ) -> Result<u64, StateStoreError> {
        let current_generation = self
            .load_account(account_id)
            .await?
            .and_then(|account| account.active_credential_generation())
            .unwrap_or(0);

        current_generation
            .checked_add(1)
            .ok_or_else(|| StateStoreError::Sqlite {
                message: "credential generation overflow".to_owned(),
            })
    }

    /// Activates one credential generation and invalidates quota selector state.
    pub async fn activate_account_credential_generation_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let active_generation = u64_to_i64(active_credential_generation)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(
            "UPDATE accounts
                SET status = ?2,
                    active_credential_generation = ?3
              WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .bind(status.as_str())
        .bind(active_generation)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        crate::credit_store::invalidate_credit_observation_after_credential_mutation(
            &mut transaction,
            account_id,
            active_credential_generation,
        )
        .await?;
        sqlx::query("DELETE FROM credential_maintenance WHERE account_id = ?1")
            .bind(account_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
        for route_band in CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS {
            sqlx::query(
                "INSERT INTO quota_snapshots (
                   account_id, source, observed_unix_seconds, route_band,
                   remaining_headroom, reset_unix_seconds, stale_penalty
                 )
                 VALUES (?1, ?2, 0, ?3, 0, NULL, 1)
                 ON CONFLICT(account_id, route_band) DO UPDATE SET
                   source = excluded.source,
                   observed_unix_seconds = excluded.observed_unix_seconds,
                   remaining_headroom = excluded.remaining_headroom,
                   reset_unix_seconds = excluded.reset_unix_seconds,
                   stale_penalty = excluded.stale_penalty",
            )
            .bind(account_id.as_str())
            .bind(QuotaSnapshotSource::CredentialMutation.as_str())
            .bind(*route_band)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
            sqlx::query(
                "DELETE FROM selector_quota_windows
                  WHERE account_id = ?1 AND route_band = ?2",
            )
            .bind(account_id.as_str())
            .bind(*route_band)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
            if selector_route_band(route_band) {
                sqlx::query(
                    "INSERT INTO selector_quota_windows (
                       account_id, route_band, limit_window_seconds, status,
                       remaining_headroom, reset_unix_seconds, effective,
                       observed_unix_seconds
                     )
                     VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, 0)",
                )
                .bind(account_id.as_str())
                .bind(*route_band)
                .bind(u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?)
                .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?;
            }
        }
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Activates one credential generation only if account state still matches the caller's read.
    pub async fn activate_account_credential_generation_if_current_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        expected_active_credential_generation: u64,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let expected_generation = u64_to_i64(expected_active_credential_generation)?;
        let active_generation = u64_to_i64(active_credential_generation)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let update = sqlx::query(
            "UPDATE accounts
                SET status = ?2,
                    active_credential_generation = ?3
              WHERE account_id = ?1
                AND status = ?4
                AND active_credential_generation = ?5",
        )
        .bind(account_id.as_str())
        .bind(status.as_str())
        .bind(active_generation)
        .bind(AccountStatus::Enabled.as_str())
        .bind(expected_generation)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        if update.rows_affected() != 1 {
            transaction.rollback().await.map_err(sqlx_error)?;
            return Err(StateStoreError::AccountConcurrentModification {
                account_id: account_id.as_str().to_owned(),
            });
        }
        crate::credit_store::invalidate_credit_observation_after_credential_mutation(
            &mut transaction,
            account_id,
            active_credential_generation,
        )
        .await?;
        invalidate_credential_mutation_quota_async(&mut transaction, account_id).await?;
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Disables an account only when the rejected credential generation is still current.
    pub async fn disable_account_if_credential_generation_current(
        &self,
        account_id: &AccountId,
        expected_active_credential_generation: u64,
    ) -> Result<bool, StateStoreError> {
        let expected_generation = u64_to_i64(expected_active_credential_generation)?;
        let update = sqlx::query(
            "UPDATE accounts
                SET status = ?2
              WHERE account_id = ?1
                AND status = ?3
                AND active_credential_generation = ?4",
        )
        .bind(account_id.as_str())
        .bind(AccountStatus::Disabled.as_str())
        .bind(AccountStatus::Enabled.as_str())
        .bind(expected_generation)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(update.rows_affected() == 1)
    }
}

fn parse_account_routing_policy_row(
    row: SqliteRow,
) -> Result<AccountRoutingPolicy, StateStoreError> {
    let account_id = AccountId::new(row.get::<String, _>(0))
        .map_err(|_| StateStoreError::CorruptAccountRoutingPolicy)?;
    let basis_points = row.get::<i64, _>(1);
    let basis_points = u16::try_from(basis_points)
        .ok()
        .and_then(|value| WeeklyQuotaFloorBasisPoints::new(value).ok())
        .ok_or(StateStoreError::CorruptAccountRoutingPolicy)?;
    Ok(AccountRoutingPolicy::new(account_id, basis_points))
}

pub(super) fn parse_account_row(
    account_id_value: String,
    label: String,
    status_value: String,
    active_credential_generation: Option<i64>,
    provider_value: String,
) -> Result<AccountRecord, StateStoreError> {
    let parsed_account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let status = AccountStatus::parse(&status_value).ok_or(StateStoreError::CorruptAccount {
        account_id: account_id_value.clone(),
        field: "status",
    })?;
    let provider = Provider::parse(&provider_value).ok_or(StateStoreError::CorruptAccount {
        account_id: account_id_value,
        field: "provider",
    })?;
    let active_credential_generation = active_credential_generation
        .map(|value| {
            i64_to_u64_account_generation(
                value,
                parsed_account_id.as_str(),
                "active_credential_generation",
            )
        })
        .transpose()?;

    let mut account = AccountRecord::new(provider, parsed_account_id, label, status);
    if let Some(generation) = active_credential_generation {
        account = account.with_active_credential_generation(generation);
    }

    Ok(account)
}

pub(crate) async fn invalidate_credential_mutation_quota_async(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: &AccountId,
) -> Result<(), StateStoreError> {
    for route_band in CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS {
        sqlx::query(
            "INSERT INTO quota_snapshots (
               account_id, source, observed_unix_seconds, route_band,
               remaining_headroom, reset_unix_seconds, stale_penalty
             )
             VALUES (?1, ?2, 0, ?3, 0, NULL, 1)
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               source = excluded.source,
               observed_unix_seconds = excluded.observed_unix_seconds,
               remaining_headroom = excluded.remaining_headroom,
               reset_unix_seconds = excluded.reset_unix_seconds,
               stale_penalty = excluded.stale_penalty",
        )
        .bind(account_id.as_str())
        .bind(QuotaSnapshotSource::CredentialMutation.as_str())
        .bind(*route_band)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(
            "DELETE FROM selector_quota_windows
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(*route_band)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_error)?;
        if selector_route_band(route_band) {
            sqlx::query(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, 0)",
            )
            .bind(account_id.as_str())
            .bind(*route_band)
            .bind(u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?)
            .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(sqlx_error)?;
        }
    }

    Ok(())
}
