//! SQLite sync snapshots responsibilities.
use super::quota_snapshots::selector_window_from_snapshot;
use super::*;

impl SqliteStateStore {
    /// Inserts or updates a persisted quota snapshot.
    pub fn upsert_quota_snapshot(
        &self,
        snapshot: &PersistedQuotaSnapshot,
    ) -> Result<(), StateStoreError> {
        let observed_unix_seconds = u64_to_i64(snapshot.observed_unix_seconds())?;
        let remaining_headroom = u32_to_i64(snapshot.remaining_headroom());
        let reset_unix_seconds = snapshot.reset_unix_seconds().map(u64_to_i64).transpose()?;
        let reset_credits_available = snapshot.reset_credits_available().map(u32_to_i64);
        let stale_penalty = if snapshot.stale_penalty() {
            1_i64
        } else {
            0_i64
        };

        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO quota_snapshots (
                   account_id, source, observed_unix_seconds, route_band,
                   remaining_headroom, reset_unix_seconds,
                   reset_credits_available, stale_penalty
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, route_band) DO UPDATE SET
                   source = excluded.source,
                   observed_unix_seconds = excluded.observed_unix_seconds,
                   remaining_headroom = excluded.remaining_headroom,
                   reset_unix_seconds = excluded.reset_unix_seconds,
                   reset_credits_available = excluded.reset_credits_available,
                   stale_penalty = excluded.stale_penalty",
                params![
                    snapshot.account_id().as_str(),
                    snapshot.source().as_str(),
                    observed_unix_seconds,
                    snapshot.route_band(),
                    remaining_headroom,
                    reset_unix_seconds,
                    reset_credits_available,
                    stale_penalty,
                ],
            )
            .map_err(sqlite_error)?;
        if selector_route_band(snapshot.route_band()) {
            let selector_window = selector_window_from_snapshot(snapshot);
            transaction
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
                        selector_window.account_id().as_str(),
                        selector_window.route_band(),
                        u64_to_i64(selector_window.limit_window_seconds())?,
                        selector_window.status().as_str(),
                        u32_to_i64(selector_window.remaining_headroom()),
                        selector_window
                            .reset_unix_seconds()
                            .map(u64_to_i64)
                            .transpose()?,
                        if selector_window.effective() {
                            1_i64
                        } else {
                            0_i64
                        },
                        u64_to_i64(selector_window.observed_unix_seconds())?,
                    ],
                )
                .map_err(sqlite_error)?;
        }
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    /// Loads a persisted quota snapshot.
    pub fn load_quota_snapshot(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT route_band
                   FROM quota_snapshots
                  WHERE account_id = ?1
                  ORDER BY route_band
                  LIMIT 1",
                params![account_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some(route_band) = row else {
            return Ok(None);
        };

        self.load_quota_snapshot_for_route_band(account_id, &route_band)
    }

    /// Loads a persisted quota snapshot for one route band.
    pub fn load_quota_snapshot_for_route_band(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT account_id, source, observed_unix_seconds, route_band,
                        remaining_headroom, reset_unix_seconds,
                        reset_credits_available, stale_penalty
                   FROM quota_snapshots
                  WHERE account_id = ?1 AND route_band = ?2",
                params![account_id.as_str(), route_band],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, Option<i64>>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some((
            account_id_value,
            source_value,
            observed_unix_seconds,
            route_band,
            remaining_headroom,
            reset_unix_seconds,
            reset_credits_available,
            stale_penalty,
        )) = row
        else {
            return Ok(None);
        };

        let parsed_account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
            StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value.clone(),
                field: "account_id",
            }
        })?;
        let source = QuotaSnapshotSource::parse(&source_value).ok_or_else(|| {
            StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value.clone(),
                field: "source",
            }
        })?;
        let observed = i64_to_u64(observed_unix_seconds, &account_id_value, "observed")?;
        let remaining = i64_to_u32(remaining_headroom, &account_id_value, "remaining_headroom")?;
        let reset = reset_unix_seconds
            .map(|value| i64_to_u64(value, &account_id_value, "reset_unix_seconds"))
            .transpose()?;
        let reset_credits = reset_credits_available
            .map(|value| i64_to_u32(value, &account_id_value, "reset_credits_available"))
            .transpose()?;
        let stale = match stale_penalty {
            0 => false,
            1 => true,
            _ => {
                return Err(StateStoreError::CorruptQuotaSnapshot {
                    account_id: account_id_value,
                    field: "stale_penalty",
                });
            }
        };

        let mut snapshot = PersistedQuotaSnapshot::new(parsed_account_id, source)
            .with_observed_unix_seconds(observed)
            .with_route_band(route_band, remaining)
            .with_stale_penalty(stale);
        if let Some(reset) = reset {
            snapshot = snapshot.with_reset_unix_seconds(reset);
        }
        if let Some(reset_credits) = reset_credits {
            snapshot = snapshot.with_reset_credits_available(reset_credits);
        }

        Ok(Some(snapshot))
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
impl QuotaSnapshotRepository for SqliteStateStore {
    fn upsert_snapshot(&self, snapshot: &PersistedQuotaSnapshot) -> Result<(), StateStoreError> {
        self.upsert_quota_snapshot(snapshot)
    }

    fn load_snapshot(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError> {
        self.load_quota_snapshot(account_id)
    }

    fn load_snapshot_for_route_band(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError> {
        self.load_quota_snapshot_for_route_band(account_id, route_band)
    }
}
