//! SQLite quota history responsibilities.
use super::selector_windows::parse_refresh_error_class;
use super::*;

impl AsyncSqliteStateStore {
    /// Appends one quota history observation through the async state pool.
    pub async fn append_quota_history_observation(
        &self,
        observation: &PersistedQuotaHistoryObservation,
    ) -> Result<(), StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        insert_quota_history_observation_in_async_transaction(&mut transaction, observation)
            .await?;
        transaction.commit().await.map_err(sqlx_error)?;
        Ok(())
    }

    /// Loads quota history observations for one account/route/window/time range.
    pub async fn quota_history_observations_for_window(
        &self,
        account_id: &AccountId,
        route_band: &str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, account_label, route_band, limit_window_seconds,
                    observed_unix_seconds, remaining_headroom, reset_unix_seconds,
                    window_status, effective, refresh_source, refresh_success,
                    refresh_error_class, reset_credits_available
               FROM quota_history_observations
              WHERE account_id = ?1
                AND route_band = ?2
                AND limit_window_seconds = ?3
                AND observed_unix_seconds >= ?4
                AND observed_unix_seconds <= ?5
              ORDER BY observed_unix_seconds, id",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .bind(u64_to_i64(limit_window_seconds)?)
        .bind(u64_to_i64(observed_from_unix_seconds)?)
        .bind(u64_to_i64(observed_to_unix_seconds)?)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        rows.into_iter()
            .map(parse_quota_history_observation_row)
            .collect()
    }

    /// Purges quota history older than the given observation timestamp.
    pub async fn purge_quota_history_before(
        &self,
        observed_before_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "DELETE FROM quota_history_observations
              WHERE observed_unix_seconds < ?1",
        )
        .bind(u64_to_i64(observed_before_unix_seconds)?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }
}

pub(crate) async fn insert_quota_history_observation_in_async_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    observation: &PersistedQuotaHistoryObservation,
) -> Result<(), StateStoreError> {
    let (refresh_success, refresh_error_class) = match observation.refresh_outcome() {
        QuotaHistoryRefreshOutcome::Success => (1_i64, None),
        QuotaHistoryRefreshOutcome::Failure { error_class } => (0_i64, Some(error_class.as_str())),
    };
    sqlx::query(
        "INSERT INTO quota_history_observations (
            account_id, account_label, route_band, limit_window_seconds,
            observed_unix_seconds, remaining_headroom, reset_unix_seconds,
            window_status, effective, refresh_source, refresh_success,
            refresh_error_class, reset_credits_available
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )
    .bind(observation.account_id().as_str())
    .bind(observation.account_label())
    .bind(observation.route_band())
    .bind(u64_to_i64(observation.limit_window_seconds())?)
    .bind(u64_to_i64(observation.observed_unix_seconds())?)
    .bind(u32_to_i64(observation.remaining_headroom()))
    .bind(
        observation
            .reset_unix_seconds()
            .map(u64_to_i64)
            .transpose()?,
    )
    .bind(observation.window_status().as_str())
    .bind(if observation.effective() {
        1_i64
    } else {
        0_i64
    })
    .bind(observation.refresh_source().as_str())
    .bind(refresh_success)
    .bind(refresh_error_class)
    .bind(observation.reset_credits_available().map(u32_to_i64))
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

fn parse_quota_history_observation_row(
    row: sqlx::sqlite::SqliteRow,
) -> Result<PersistedQuotaHistoryObservation, StateStoreError> {
    let account_id_value = row.get::<String, _>(0);
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let route_band = row.get::<String, _>(2);
    let window_status_value = row.get::<String, _>(7);
    let window_status =
        SelectorQuotaWindowStatus::parse(&window_status_value).ok_or_else(|| {
            StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value.clone(),
                field: "window_status",
            }
        })?;
    let effective = match row.get::<i64, _>(8) {
        0 => false,
        1 => true,
        _ => {
            return Err(StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value,
                field: "effective",
            });
        }
    };
    let refresh_source_value = row.get::<String, _>(9);
    let refresh_source = QuotaSnapshotSource::parse(&refresh_source_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaSnapshot {
            account_id: account_id_value.clone(),
            field: "refresh_source",
        }
    })?;
    let refresh_success = row.get::<i64, _>(10);
    let refresh_error_class_value = row.get::<Option<String>, _>(11);
    let refresh_outcome = match (refresh_success, refresh_error_class_value.as_deref()) {
        (1, None) => QuotaHistoryRefreshOutcome::Success,
        (0, Some(error_class)) => QuotaHistoryRefreshOutcome::Failure {
            error_class: parse_refresh_error_class(error_class)?,
        },
        (0, None) => QuotaHistoryRefreshOutcome::Failure {
            error_class: QuotaRefreshErrorClass::ProviderError,
        },
        _ => {
            return Err(StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value,
                field: "refresh_success",
            });
        }
    };
    let mut observation = PersistedQuotaHistoryObservation::new(
        account_id,
        row.get::<String, _>(1),
        route_band,
        i64_to_u64(
            row.get::<i64, _>(3),
            &account_id_value,
            "limit_window_seconds",
        )?,
        i64_to_u64(
            row.get::<i64, _>(4),
            &account_id_value,
            "observed_unix_seconds",
        )?,
        i64_to_u32(
            row.get::<i64, _>(5),
            &account_id_value,
            "remaining_headroom",
        )?,
    )
    .with_window_status(window_status)
    .with_effective(effective)
    .with_refresh_source(refresh_source)
    .with_refresh_outcome(refresh_outcome);
    if let Some(reset_unix_seconds) = row
        .get::<Option<i64>, _>(6)
        .map(|value| i64_to_u64(value, &account_id_value, "reset_unix_seconds"))
        .transpose()?
    {
        observation = observation.with_reset_unix_seconds(reset_unix_seconds);
    }
    if let Some(reset_credits_available) = row
        .get::<Option<i64>, _>(12)
        .map(|value| i64_to_u32(value, &account_id_value, "reset_credits_available"))
        .transpose()?
    {
        observation = observation.with_reset_credits_available(reset_credits_available);
    }

    Ok(observation)
}
