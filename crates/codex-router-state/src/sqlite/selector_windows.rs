//! SQLite selector windows responsibilities.
use super::*;
pub(super) const DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS: u64 = 18_000;

pub(super) const WEEKLY_SELECTOR_LIMIT_WINDOW_SECONDS: u64 = 604_800;

pub(super) const SUSPECT_EXHAUSTED_TTL_SECONDS: u64 = 300;

pub(super) const SELECTOR_INVALIDATED_ROUTE_BANDS: [&str; 4] = [
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct RouteBandAccountStateRow {
    state: String,
    reason_code: String,
    observed_unix_seconds: u64,
    expires_unix_seconds: Option<u64>,
}

impl RouteBandAccountStateRow {
    fn is_active_suspect_exhausted(&self, now_unix_seconds: u64) -> bool {
        self.state == "suspect_exhausted"
            && !self.reason_code.is_empty()
            && !self.is_expired(now_unix_seconds)
    }

    fn is_expired(&self, now_unix_seconds: u64) -> bool {
        self.expires_unix_seconds
            .is_some_and(|expires_unix_seconds| now_unix_seconds >= expires_unix_seconds)
    }
}

impl AsyncSqliteStateStore {
    /// Loads selector input rows for one route band.
    pub async fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let account_rows = sqlx::query!(
            "SELECT account_id, label, status, active_credential_generation, provider
               FROM accounts
              ORDER BY account_id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        let accounts = account_rows
            .into_iter()
            .map(|row| {
                parse_account_row(
                    row.account_id,
                    row.label,
                    row.status,
                    row.active_credential_generation,
                    row.provider,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut inputs = Vec::new();
        for account in accounts {
            let mut windows = self
                .load_selector_windows(&mut transaction, account.account_id(), route_band)
                .await?;
            let route_band_state = self
                .load_route_band_account_state(&mut transaction, account.account_id(), route_band)
                .await?;
            let mut suspect_exhausted_credit_suppression = false;
            if let Some(state) = route_band_state.as_ref() {
                if state.is_active_suspect_exhausted(now_unix_seconds) {
                    suspect_exhausted_credit_suppression = true;
                    windows = suspect_exhausted_selector_windows(
                        account.account_id(),
                        route_band,
                        state.observed_unix_seconds,
                    );
                } else if state.is_expired(now_unix_seconds) {
                    windows.clear();
                }
            } else {
                let refresh_status = self
                    .load_quota_refresh_status(&mut transaction, account.account_id(), route_band)
                    .await?;
                if selector_windows_are_stale(&windows, refresh_status.as_ref(), now_unix_seconds) {
                    mark_selector_windows_stale(&mut windows);
                }
            }
            let (window_observations, window_rejections) = if account.provider() == Provider::Claude
            {
                (
                    crate::window_observation::window_observations_for_account_in_transaction(
                        &mut transaction,
                        account.account_id(),
                    )
                    .await?,
                    crate::window_observation::window_rejections_for_account_in_transaction(
                        &mut transaction,
                        account.account_id(),
                    )
                    .await?,
                )
            } else {
                (Vec::new(), Vec::new())
            };
            let (credit_usage_policy, credit_observation) =
                crate::credit_store::load_credit_usage_for_selector(
                    &mut transaction,
                    account.account_id(),
                )
                .await?;
            let canonical_responses_windows = if route_band == RouteBand::ResponsesCompact.as_str()
                && account.provider() == Provider::Openai
                && credit_usage_policy.allows_credit_usage()
            {
                let canonical_route_band = RouteBand::Responses.as_str();
                let mut canonical_windows = self
                    .load_selector_windows(
                        &mut transaction,
                        account.account_id(),
                        canonical_route_band,
                    )
                    .await?;
                let canonical_route_band_state = self
                    .load_route_band_account_state(
                        &mut transaction,
                        account.account_id(),
                        canonical_route_band,
                    )
                    .await?;
                if let Some(state) = canonical_route_band_state.as_ref() {
                    if state.is_active_suspect_exhausted(now_unix_seconds) {
                        suspect_exhausted_credit_suppression = true;
                        canonical_windows = suspect_exhausted_selector_windows(
                            account.account_id(),
                            canonical_route_band,
                            state.observed_unix_seconds,
                        );
                    } else if state.is_expired(now_unix_seconds) {
                        canonical_windows.clear();
                    }
                } else {
                    let canonical_refresh_status = self
                        .load_quota_refresh_status(
                            &mut transaction,
                            account.account_id(),
                            canonical_route_band,
                        )
                        .await?;
                    if selector_windows_are_stale(
                        &canonical_windows,
                        canonical_refresh_status.as_ref(),
                        now_unix_seconds,
                    ) {
                        mark_selector_windows_stale(&mut canonical_windows);
                    }
                }
                Some(canonical_windows)
            } else {
                None
            };
            let credential_maintenance = if account.provider() == Provider::Claude {
                self.load_credential_maintenance_in_transaction(
                    account.account_id(),
                    &mut transaction,
                )
                .await?
                .map(|record| {
                    SelectorCredentialMaintenance::new(record.credential_generation, record.state)
                })
            } else {
                None
            };
            inputs.push(
                SelectorQuotaInput::new(
                    account.account_id().clone(),
                    account.label(),
                    account.provider(),
                    account.status(),
                    account.active_credential_generation(),
                    route_band,
                    windows,
                )
                .with_credential_maintenance(credential_maintenance)
                .with_window_state(window_observations, window_rejections)
                .with_canonical_responses_windows(canonical_responses_windows)
                .with_credit_usage(
                    credit_usage_policy,
                    credit_observation,
                    suspect_exhausted_credit_suppression,
                ),
            );
        }

        transaction.commit().await.map_err(sqlx_error)?;
        Ok(inputs)
    }

    /// Marks an account's route-band quota as exhausted for future selector reads.
    pub async fn mark_route_band_quota_exhausted(
        &self,
        account_id: &AccountId,
        route_band: &str,
        observed_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let observed_unix_seconds_value = observed_unix_seconds;
        let observed_unix_seconds = u64_to_i64(observed_unix_seconds)?;
        let expires_unix_seconds =
            u64_to_i64(observed_unix_seconds_value + SUSPECT_EXHAUSTED_TTL_SECONDS)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(
            "INSERT INTO route_band_account_states (
               account_id, route_band, state, reason_code,
               observed_unix_seconds, expires_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               state = excluded.state,
               reason_code = excluded.reason_code,
               observed_unix_seconds = excluded.observed_unix_seconds,
               expires_unix_seconds = excluded.expires_unix_seconds",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .bind("suspect_exhausted")
        .bind("provider_quota_exhausted")
        .bind(observed_unix_seconds)
        .bind(expires_unix_seconds)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(
            "INSERT INTO quota_snapshots (
               account_id, source, observed_unix_seconds, route_band,
               remaining_headroom, reset_unix_seconds, stale_penalty
             )
             VALUES (?1, ?2, ?3, ?4, 0, NULL, 0)
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               source = excluded.source,
               observed_unix_seconds = excluded.observed_unix_seconds,
               remaining_headroom = excluded.remaining_headroom,
               reset_unix_seconds = excluded.reset_unix_seconds,
               stale_penalty = excluded.stale_penalty",
        )
        .bind(account_id.as_str())
        .bind(QuotaSnapshotSource::OpenAiEndpoint.as_str())
        .bind(observed_unix_seconds)
        .bind(route_band)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(
            "UPDATE selector_quota_windows
                SET status = ?3,
                    remaining_headroom = 0,
                    observed_unix_seconds = ?4
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
        .bind(observed_unix_seconds)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        if selector_route_band(route_band) {
            for window in suspect_exhausted_selector_windows(
                account_id,
                route_band,
                observed_unix_seconds_value,
            ) {
                insert_selector_window_in_async_transaction(&mut transaction, &window).await?;
            }
        }
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    async fn load_quota_refresh_status(
        &self,
        connection: &mut sqlx::SqliteConnection,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<QuotaRefreshStatusView>, StateStoreError> {
        let row = sqlx::query(
            "SELECT last_success_unix_seconds, last_attempt_unix_seconds,
                    last_error_class, stale_after_unix_seconds
               FROM quota_refresh_status
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .fetch_optional(&mut *connection)
        .await
        .map_err(sqlx_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        Ok(Some(QuotaRefreshStatusView::recorded(
            account_id.clone(),
            route_band,
            row.get::<Option<i64>, _>(0)
                .map(|value| i64_to_u64(value, account_id.as_str(), "last_success_unix_seconds"))
                .transpose()?,
            row.get::<Option<i64>, _>(1)
                .map(|value| i64_to_u64(value, account_id.as_str(), "last_attempt_unix_seconds"))
                .transpose()?,
            row.get::<Option<String>, _>(2)
                .as_deref()
                .map(parse_refresh_error_class)
                .transpose()?,
            row.get::<Option<i64>, _>(3)
                .map(|value| i64_to_u64(value, account_id.as_str(), "stale_after_unix_seconds"))
                .transpose()?,
        )))
    }

    /// Inserts or updates one selector quota window through the async state pool.
    pub async fn upsert_selector_quota_window(
        &self,
        window: &PersistedSelectorQuotaWindow,
    ) -> Result<(), StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        insert_selector_window_in_async_transaction(&mut transaction, window).await?;
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Atomically records a successful refresh and replaces selector windows.
    pub async fn record_refresh_success_and_replace_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        windows: &[PersistedSelectorQuotaWindow],
        last_success_unix_seconds: u64,
        stale_after_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let last_success = u64_to_i64(last_success_unix_seconds)?;
        let stale_after = u64_to_i64(stale_after_unix_seconds)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(
            "DELETE FROM selector_quota_windows
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(
            "DELETE FROM route_band_account_states
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        for window in windows {
            insert_selector_window_in_async_transaction(&mut transaction, window).await?;
        }
        super::quota_refresh_status::record_refresh_success_status_in_transaction(
            &mut transaction,
            account_id,
            route_band,
            last_success,
            stale_after,
        )
        .await?;
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Atomically records a failed refresh while preserving selector windows.
    pub async fn record_refresh_failure_preserving_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        last_attempt_unix_seconds: u64,
        last_error_class: QuotaRefreshErrorClass,
    ) -> Result<(), StateStoreError> {
        let last_attempt = u64_to_i64(last_attempt_unix_seconds)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(
            "INSERT INTO quota_refresh_status (
               account_id, route_band, last_success_unix_seconds,
               last_attempt_unix_seconds, last_error_class,
               stale_after_unix_seconds
             )
             VALUES (?1, ?2, NULL, ?3, ?4, ?3)
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
                 END",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .bind(last_attempt)
        .bind(last_error_class.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Loads refresh status view rows for one route band through the async state pool.
    pub async fn quota_refresh_statuses_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<QuotaRefreshStatusView>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT
               accounts.account_id,
               quota_refresh_status.last_success_unix_seconds,
               quota_refresh_status.last_attempt_unix_seconds,
               quota_refresh_status.last_error_class,
               quota_refresh_status.stale_after_unix_seconds,
               CASE
                 WHEN quota_refresh_status.account_id IS NULL THEN 0
                 ELSE 1
               END
             FROM accounts
             LEFT JOIN quota_refresh_status
               ON quota_refresh_status.account_id = accounts.account_id
              AND quota_refresh_status.route_band = ?1
             WHERE quota_refresh_status.account_id IS NOT NULL
                OR EXISTS (
                     SELECT 1 FROM selector_quota_windows
                      WHERE selector_quota_windows.account_id = accounts.account_id
                        AND selector_quota_windows.route_band = ?1
                   )
             ORDER BY accounts.account_id",
        )
        .bind(route_band)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut statuses = Vec::new();
        for row in rows {
            statuses.push(parse_quota_refresh_status_row(
                route_band,
                row.get::<String, _>(0),
                row.get::<Option<i64>, _>(1),
                row.get::<Option<i64>, _>(2),
                row.get::<Option<String>, _>(3),
                row.get::<Option<i64>, _>(4),
                row.get::<i64, _>(5),
            )?);
        }

        Ok(statuses)
    }

    async fn load_selector_windows(
        &self,
        connection: &mut sqlx::SqliteConnection,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Vec<PersistedSelectorQuotaWindow>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, route_band, limit_window_seconds, status,
                    remaining_headroom, reset_unix_seconds, effective,
                    observed_unix_seconds
               FROM selector_quota_windows
              WHERE account_id = ?1 AND route_band = ?2
              ORDER BY effective DESC, limit_window_seconds",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .fetch_all(&mut *connection)
        .await
        .map_err(sqlx_error)?;

        let mut windows = Vec::new();
        for row in rows {
            let account_id_value = row.get::<String, _>(0);
            let status_value = row.get::<String, _>(3);
            let parsed_account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptQuotaSnapshot {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            let status = SelectorQuotaWindowStatus::parse(&status_value).ok_or_else(|| {
                StateStoreError::CorruptQuotaSnapshot {
                    account_id: account_id_value.clone(),
                    field: "selector_status",
                }
            })?;
            let effective = match row.get::<i64, _>(6) {
                0 => false,
                1 => true,
                _ => {
                    return Err(StateStoreError::CorruptQuotaSnapshot {
                        account_id: account_id_value,
                        field: "effective",
                    });
                }
            };
            let mut window = PersistedSelectorQuotaWindow::new(
                parsed_account_id,
                row.get::<String, _>(1),
                i64_to_u64(
                    row.get::<i64, _>(2),
                    &account_id_value,
                    "limit_window_seconds",
                )?,
                status,
            )
            .with_remaining_headroom(i64_to_u32(
                row.get::<i64, _>(4),
                &account_id_value,
                "remaining_headroom",
            )?)
            .with_effective(effective)
            .with_observed_unix_seconds(i64_to_u64(
                row.get::<i64, _>(7),
                &account_id_value,
                "observed_unix_seconds",
            )?);
            if let Some(reset) = row
                .get::<Option<i64>, _>(5)
                .map(|value| i64_to_u64(value, &account_id_value, "reset_unix_seconds"))
                .transpose()?
            {
                window = window.with_reset_unix_seconds(reset);
            }
            windows.push(window);
        }

        Ok(windows)
    }

    async fn load_route_band_account_state(
        &self,
        connection: &mut sqlx::SqliteConnection,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<RouteBandAccountStateRow>, StateStoreError> {
        let row = sqlx::query(
            "SELECT state, reason_code, observed_unix_seconds, expires_unix_seconds
               FROM route_band_account_states
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .fetch_optional(&mut *connection)
        .await
        .map_err(sqlx_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        let observed_unix_seconds = i64_to_u64(
            row.get::<i64, _>(2),
            account_id.as_str(),
            "observed_unix_seconds",
        )?;
        let expires_unix_seconds = row
            .get::<Option<i64>, _>(3)
            .map(|value| i64_to_u64(value, account_id.as_str(), "expires_unix_seconds"))
            .transpose()?;

        Ok(Some(RouteBandAccountStateRow {
            state: row.get::<String, _>(0),
            reason_code: row.get::<String, _>(1),
            observed_unix_seconds,
            expires_unix_seconds,
        }))
    }
}

pub(super) async fn insert_selector_window_in_async_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    window: &PersistedSelectorQuotaWindow,
) -> Result<(), StateStoreError> {
    sqlx::query(
        "INSERT INTO selector_quota_windows (
           account_id, route_band, limit_window_seconds, status,
           remaining_headroom, reset_unix_seconds, effective,
           observed_unix_seconds
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(account_id, route_band, limit_window_seconds) DO UPDATE SET
           status = excluded.status,
           remaining_headroom = excluded.remaining_headroom,
           reset_unix_seconds = excluded.reset_unix_seconds,
           effective = excluded.effective,
           observed_unix_seconds = excluded.observed_unix_seconds",
    )
    .bind(window.account_id().as_str())
    .bind(window.route_band())
    .bind(u64_to_i64(window.limit_window_seconds())?)
    .bind(window.status().as_str())
    .bind(u32_to_i64(window.remaining_headroom()))
    .bind(window.reset_unix_seconds().map(u64_to_i64).transpose()?)
    .bind(if window.effective() { 1_i64 } else { 0_i64 })
    .bind(u64_to_i64(window.observed_unix_seconds())?)
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_error)?;

    Ok(())
}

pub(super) fn selector_windows_are_stale(
    windows: &[PersistedSelectorQuotaWindow],
    refresh_status: Option<&QuotaRefreshStatusView>,
    now_unix_seconds: u64,
) -> bool {
    if windows.is_empty() {
        return false;
    }

    let Some(refresh_status) = refresh_status else {
        return true;
    };
    let Some(stale_after_unix_seconds) = refresh_status.stale_after_unix_seconds() else {
        return true;
    };

    now_unix_seconds >= stale_after_unix_seconds
}

pub(super) fn mark_selector_windows_stale(windows: &mut [PersistedSelectorQuotaWindow]) {
    for window in windows {
        if window.status() != SelectorQuotaWindowStatus::Eligible {
            continue;
        }
        let mut stale_window = PersistedSelectorQuotaWindow::new(
            window.account_id().clone(),
            window.route_band(),
            window.limit_window_seconds(),
            SelectorQuotaWindowStatus::Stale,
        )
        .with_remaining_headroom(window.remaining_headroom())
        .with_effective(window.effective())
        .with_observed_unix_seconds(window.observed_unix_seconds());
        if let Some(reset_unix_seconds) = window.reset_unix_seconds() {
            stale_window = stale_window.with_reset_unix_seconds(reset_unix_seconds);
        }
        *window = stale_window;
    }
}

fn suspect_exhausted_selector_windows(
    account_id: &AccountId,
    route_band: &str,
    observed_unix_seconds: u64,
) -> Vec<PersistedSelectorQuotaWindow> {
    [
        (DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS, true),
        (WEEKLY_SELECTOR_LIMIT_WINDOW_SECONDS, false),
    ]
    .into_iter()
    .map(|(limit_window_seconds, effective)| {
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            route_band,
            limit_window_seconds,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(effective)
        .with_observed_unix_seconds(observed_unix_seconds)
    })
    .collect()
}

pub(super) fn parse_refresh_error_class(
    value: &str,
) -> Result<QuotaRefreshErrorClass, StateStoreError> {
    QuotaRefreshErrorClass::parse(value).ok_or_else(|| StateStoreError::CorruptQuotaSnapshot {
        account_id: "<quota-refresh-status>".to_owned(),
        field: "last_error_class",
    })
}

fn parse_quota_refresh_status_row(
    route_band: &str,
    account_id_value: String,
    last_success_unix_seconds: Option<i64>,
    last_attempt_unix_seconds: Option<i64>,
    last_error_class: Option<String>,
    stale_after_unix_seconds: Option<i64>,
    has_recorded_status: i64,
) -> Result<QuotaRefreshStatusView, StateStoreError> {
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    if has_recorded_status == 0 {
        return Ok(QuotaRefreshStatusView::legacy_missing_refresh_status(
            account_id, route_band,
        ));
    }

    Ok(QuotaRefreshStatusView::recorded(
        account_id,
        route_band,
        last_success_unix_seconds
            .map(|value| i64_to_u64(value, &account_id_value, "last_success_unix_seconds"))
            .transpose()?,
        last_attempt_unix_seconds
            .map(|value| i64_to_u64(value, &account_id_value, "last_attempt_unix_seconds"))
            .transpose()?,
        last_error_class
            .as_deref()
            .map(parse_refresh_error_class)
            .transpose()?,
        stale_after_unix_seconds
            .map(|value| i64_to_u64(value, &account_id_value, "stale_after_unix_seconds"))
            .transpose()?,
    ))
}

#[cfg(test)]
#[path = "selector_windows_tests.rs"]
mod tests;
