//! SQLite sync selectors responsibilities.
use super::account_store::CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS;
use super::selector_windows::{
    mark_selector_windows_stale, parse_refresh_error_class, selector_windows_are_stale,
};
use super::*;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use rusqlite::Transaction;
impl SqliteStateStore {
    /// Inserts or updates a selector quota window.
    pub fn upsert_selector_quota_window(
        &self,
        window: &PersistedSelectorQuotaWindow,
    ) -> Result<(), StateStoreError> {
        let limit_window_seconds = u64_to_i64(window.limit_window_seconds())?;
        let remaining_headroom = u32_to_i64(window.remaining_headroom());
        let reset_unix_seconds = window.reset_unix_seconds().map(u64_to_i64).transpose()?;
        let effective = if window.effective() { 1_i64 } else { 0_i64 };
        let observed_unix_seconds = u64_to_i64(window.observed_unix_seconds())?;

        self.connection
            .execute(
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
                params![
                    window.account_id().as_str(),
                    window.route_band(),
                    limit_window_seconds,
                    window.status().as_str(),
                    remaining_headroom,
                    reset_unix_seconds,
                    effective,
                    observed_unix_seconds,
                ],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    /// Atomically records a successful refresh and replaces selector windows.
    pub fn record_refresh_success_and_replace_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        windows: &[PersistedSelectorQuotaWindow],
        last_success_unix_seconds: u64,
        stale_after_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let last_success = u64_to_i64(last_success_unix_seconds)?;
        let stale_after = u64_to_i64(stale_after_unix_seconds)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "DELETE FROM selector_quota_windows
                  WHERE account_id = ?1 AND route_band = ?2",
                params![account_id.as_str(), route_band],
            )
            .map_err(sqlite_error)?;
        for window in windows {
            insert_selector_window_in_transaction(&transaction, window)?;
        }
        transaction
            .execute(
                "INSERT INTO quota_refresh_status (
                   account_id, route_band, last_success_unix_seconds,
                   last_attempt_unix_seconds, last_error_class,
                   stale_after_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?3, NULL, ?4)
                 ON CONFLICT(account_id, route_band) DO UPDATE SET
                   last_success_unix_seconds = excluded.last_success_unix_seconds,
                   last_attempt_unix_seconds = excluded.last_attempt_unix_seconds,
                   last_error_class = excluded.last_error_class,
                   stale_after_unix_seconds = excluded.stale_after_unix_seconds",
                params![account_id.as_str(), route_band, last_success, stale_after],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    /// Atomically records a failed refresh while preserving selector windows.
    pub fn record_refresh_failure_preserving_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        last_attempt_unix_seconds: u64,
        last_error_class: QuotaRefreshErrorClass,
    ) -> Result<(), StateStoreError> {
        let last_attempt = u64_to_i64(last_attempt_unix_seconds)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
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
                params![
                    account_id.as_str(),
                    route_band,
                    last_attempt,
                    last_error_class.as_str(),
                ],
            )
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    /// Loads selector input rows for one route band.
    pub fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        let accounts = self.list_accounts()?;
        let mut inputs = Vec::new();
        for account in accounts {
            let mut windows = self.load_selector_windows(account.account_id(), route_band)?;
            let refresh_status =
                self.load_quota_refresh_status(account.account_id(), route_band)?;
            if selector_windows_are_stale(&windows, refresh_status.as_ref(), now_unix_seconds) {
                mark_selector_windows_stale(&mut windows);
            }
            inputs.push(SelectorQuotaInput::new(
                account.account_id().clone(),
                account.label(),
                account.provider(),
                account.status(),
                account.active_credential_generation(),
                route_band,
                windows,
            ));
        }

        Ok(inputs)
    }

    /// Loads refresh status view rows for one route band.
    pub fn quota_refresh_statuses_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<QuotaRefreshStatusView>, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
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
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![route_band], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut statuses = Vec::new();
        for row in rows {
            let (
                account_id_value,
                last_success_unix_seconds,
                last_attempt_unix_seconds,
                last_error_class,
                stale_after_unix_seconds,
                has_recorded_status,
            ) = row.map_err(sqlite_error)?;
            let account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
                StateStoreError::CorruptAccount {
                    account_id: account_id_value.clone(),
                    field: "account_id",
                }
            })?;
            if has_recorded_status == 0 {
                statuses.push(QuotaRefreshStatusView::legacy_missing_refresh_status(
                    account_id, route_band,
                ));
                continue;
            }

            statuses.push(QuotaRefreshStatusView::recorded(
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
            ));
        }

        Ok(statuses)
    }

    fn load_quota_refresh_status(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<QuotaRefreshStatusView>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT last_success_unix_seconds, last_attempt_unix_seconds,
                        last_error_class, stale_after_unix_seconds
                   FROM quota_refresh_status
                  WHERE account_id = ?1 AND route_band = ?2",
                params![account_id.as_str(), route_band],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some((
            last_success_unix_seconds,
            last_attempt_unix_seconds,
            last_error_class,
            stale_after_unix_seconds,
        )) = row
        else {
            return Ok(None);
        };

        Ok(Some(QuotaRefreshStatusView::recorded(
            account_id.clone(),
            route_band,
            last_success_unix_seconds
                .map(|value| i64_to_u64(value, account_id.as_str(), "last_success_unix_seconds"))
                .transpose()?,
            last_attempt_unix_seconds
                .map(|value| i64_to_u64(value, account_id.as_str(), "last_attempt_unix_seconds"))
                .transpose()?,
            last_error_class
                .as_deref()
                .map(parse_refresh_error_class)
                .transpose()?,
            stale_after_unix_seconds
                .map(|value| i64_to_u64(value, account_id.as_str(), "stale_after_unix_seconds"))
                .transpose()?,
        )))
    }

    fn load_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Vec<PersistedSelectorQuotaWindow>, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT account_id, route_band, limit_window_seconds, status,
                        remaining_headroom, reset_unix_seconds, effective,
                        observed_unix_seconds
                   FROM selector_quota_windows
                  WHERE account_id = ?1 AND route_band = ?2
                  ORDER BY effective DESC, limit_window_seconds",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![account_id.as_str(), route_band], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut windows = Vec::new();
        for row in rows {
            let (
                account_id_value,
                route_band,
                limit_window_seconds,
                status_value,
                remaining_headroom,
                reset_unix_seconds,
                effective,
                observed_unix_seconds,
            ) = row.map_err(sqlite_error)?;
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
            let limit_window_seconds = i64_to_u64(
                limit_window_seconds,
                &account_id_value,
                "limit_window_seconds",
            )?;
            let remaining =
                i64_to_u32(remaining_headroom, &account_id_value, "remaining_headroom")?;
            let reset = reset_unix_seconds
                .map(|value| i64_to_u64(value, &account_id_value, "reset_unix_seconds"))
                .transpose()?;
            let effective = match effective {
                0 => false,
                1 => true,
                _ => {
                    return Err(StateStoreError::CorruptQuotaSnapshot {
                        account_id: account_id_value,
                        field: "effective",
                    });
                }
            };
            let observed = i64_to_u64(
                observed_unix_seconds,
                &account_id_value,
                "observed_unix_seconds",
            )?;
            let mut window = PersistedSelectorQuotaWindow::new(
                parsed_account_id,
                route_band,
                limit_window_seconds,
                status,
            )
            .with_remaining_headroom(remaining)
            .with_effective(effective)
            .with_observed_unix_seconds(observed);
            if let Some(reset) = reset {
                window = window.with_reset_unix_seconds(reset);
            }
            windows.push(window);
        }

        Ok(windows)
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
impl SelectorQuotaRepository for SqliteStateStore {
    fn upsert_selector_window(
        &self,
        window: &PersistedSelectorQuotaWindow,
    ) -> Result<(), StateStoreError> {
        self.upsert_selector_quota_window(window)
    }

    fn record_refresh_success_and_replace_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        windows: &[PersistedSelectorQuotaWindow],
        last_success_unix_seconds: u64,
        stale_after_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        self.record_refresh_success_and_replace_selector_windows(
            account_id,
            route_band,
            windows,
            last_success_unix_seconds,
            stale_after_unix_seconds,
        )
    }

    fn record_refresh_failure_preserving_selector_windows(
        &self,
        account_id: &AccountId,
        route_band: &str,
        last_attempt_unix_seconds: u64,
        last_error_class: QuotaRefreshErrorClass,
    ) -> Result<(), StateStoreError> {
        self.record_refresh_failure_preserving_selector_windows(
            account_id,
            route_band,
            last_attempt_unix_seconds,
            last_error_class,
        )
    }

    fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        self.selector_inputs_for_route_band(route_band, now_unix_seconds)
    }

    fn quota_refresh_statuses_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<QuotaRefreshStatusView>, StateStoreError> {
        self.quota_refresh_statuses_for_route_band(route_band)
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
fn insert_selector_window_in_transaction(
    transaction: &Transaction<'_>,
    window: &PersistedSelectorQuotaWindow,
) -> Result<(), StateStoreError> {
    transaction
        .execute(
            "INSERT INTO selector_quota_windows (
               account_id, route_band, limit_window_seconds, status,
               remaining_headroom, reset_unix_seconds, effective,
               observed_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                window.account_id().as_str(),
                window.route_band(),
                u64_to_i64(window.limit_window_seconds())?,
                window.status().as_str(),
                u32_to_i64(window.remaining_headroom()),
                window.reset_unix_seconds().map(u64_to_i64).transpose()?,
                if window.effective() { 1_i64 } else { 0_i64 },
                u64_to_i64(window.observed_unix_seconds())?,
            ],
        )
        .map_err(sqlite_error)?;

    Ok(())
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) fn invalidate_credential_mutation_quota_sync(
    transaction: &Transaction<'_>,
    account_id: &AccountId,
) -> Result<(), StateStoreError> {
    for route_band in CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS {
        transaction
            .execute(
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
                params![
                    account_id.as_str(),
                    QuotaSnapshotSource::CredentialMutation.as_str(),
                    route_band,
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "DELETE FROM selector_quota_windows
                  WHERE account_id = ?1 AND route_band = ?2",
                params![account_id.as_str(), route_band],
            )
            .map_err(sqlite_error)?;
        if selector_route_band(route_band) {
            transaction
                .execute(
                    "INSERT INTO selector_quota_windows (
                       account_id, route_band, limit_window_seconds, status,
                       remaining_headroom, reset_unix_seconds, effective,
                       observed_unix_seconds
                     )
                     VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, 0)",
                    params![
                        account_id.as_str(),
                        route_band,
                        u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?,
                        SelectorQuotaWindowStatus::Ineligible.as_str(),
                    ],
                )
                .map_err(sqlite_error)?;
        }
    }

    Ok(())
}
