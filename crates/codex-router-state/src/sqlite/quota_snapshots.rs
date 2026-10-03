//! SQLite quota snapshots responsibilities.
use super::*;
#[derive(Clone, Copy, Eq, PartialEq)]
enum SnapshotSelectorProjection {
    DeriveFromSnapshot,
    PreserveProviderWindows,
}

impl AsyncSqliteStateStore {
    /// Inserts or updates a persisted quota snapshot through the async state pool.
    pub async fn upsert_quota_snapshot(
        &self,
        snapshot: &PersistedQuotaSnapshot,
    ) -> Result<(), StateStoreError> {
        self.write_quota_snapshot(snapshot, SnapshotSelectorProjection::DeriveFromSnapshot)
            .await
    }

    /// Saves a provider snapshot after its exact selector windows were already committed.
    pub async fn upsert_quota_snapshot_preserving_selector_windows(
        &self,
        snapshot: &PersistedQuotaSnapshot,
    ) -> Result<(), StateStoreError> {
        self.write_quota_snapshot(
            snapshot,
            SnapshotSelectorProjection::PreserveProviderWindows,
        )
        .await
    }

    async fn write_quota_snapshot(
        &self,
        snapshot: &PersistedQuotaSnapshot,
        selector_projection: SnapshotSelectorProjection,
    ) -> Result<(), StateStoreError> {
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        upsert_quota_snapshot_in_async_transaction(&mut transaction, snapshot).await?;
        if selector_projection == SnapshotSelectorProjection::DeriveFromSnapshot
            && selector_route_band(snapshot.route_band())
        {
            let selector_window = selector_window_from_snapshot(snapshot);
            insert_selector_window_in_async_transaction(&mut transaction, &selector_window).await?;
        }
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Loads a persisted quota snapshot for one route band through the async state pool.
    pub async fn load_quota_snapshot_for_route_band(
        &self,
        account_id: &AccountId,
        route_band: &str,
    ) -> Result<Option<PersistedQuotaSnapshot>, StateStoreError> {
        let row = sqlx::query(
            "SELECT account_id, source, observed_unix_seconds, route_band,
                    remaining_headroom, reset_unix_seconds,
                    reset_credits_available, stale_penalty
               FROM quota_snapshots
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(route_band)
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        parse_quota_snapshot_row(QuotaSnapshotSqlRow {
            account_id_value: row.get::<String, _>(0),
            source_value: row.get::<String, _>(1),
            observed_unix_seconds: row.get::<i64, _>(2),
            route_band: row.get::<String, _>(3),
            remaining_headroom: row.get::<i64, _>(4),
            reset_unix_seconds: row.get::<Option<i64>, _>(5),
            reset_credits_available: row.get::<Option<i64>, _>(6),
            stale_penalty: row.get::<i64, _>(7),
        })
        .map(Some)
    }
}

pub(crate) async fn upsert_quota_snapshot_in_async_transaction(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    snapshot: &PersistedQuotaSnapshot,
) -> Result<(), StateStoreError> {
    let stale_penalty = if snapshot.stale_penalty() {
        1_i64
    } else {
        0_i64
    };
    sqlx::query(
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
    )
    .bind(snapshot.account_id().as_str())
    .bind(snapshot.source().as_str())
    .bind(u64_to_i64(snapshot.observed_unix_seconds())?)
    .bind(snapshot.route_band())
    .bind(u32_to_i64(snapshot.remaining_headroom()))
    .bind(snapshot.reset_unix_seconds().map(u64_to_i64).transpose()?)
    .bind(snapshot.reset_credits_available().map(u32_to_i64))
    .bind(stale_penalty)
    .execute(&mut **transaction)
    .await
    .map_err(sqlx_error)?;
    Ok(())
}

struct QuotaSnapshotSqlRow {
    account_id_value: String,
    source_value: String,
    observed_unix_seconds: i64,
    route_band: String,
    remaining_headroom: i64,
    reset_unix_seconds: Option<i64>,
    reset_credits_available: Option<i64>,
    stale_penalty: i64,
}

fn parse_quota_snapshot_row(
    row: QuotaSnapshotSqlRow,
) -> Result<PersistedQuotaSnapshot, StateStoreError> {
    let parsed_account_id = AccountId::new(row.account_id_value.clone()).map_err(|_| {
        StateStoreError::CorruptQuotaSnapshot {
            account_id: row.account_id_value.clone(),
            field: "account_id",
        }
    })?;
    let source = QuotaSnapshotSource::parse(&row.source_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaSnapshot {
            account_id: row.account_id_value.clone(),
            field: "source",
        }
    })?;
    let observed = i64_to_u64(row.observed_unix_seconds, &row.account_id_value, "observed")?;
    let remaining = i64_to_u32(
        row.remaining_headroom,
        &row.account_id_value,
        "remaining_headroom",
    )?;
    let reset = row
        .reset_unix_seconds
        .map(|value| i64_to_u64(value, &row.account_id_value, "reset_unix_seconds"))
        .transpose()?;
    let reset_credits = row
        .reset_credits_available
        .map(|value| i64_to_u32(value, &row.account_id_value, "reset_credits_available"))
        .transpose()?;
    let stale = match row.stale_penalty {
        0 => false,
        1 => true,
        _ => {
            return Err(StateStoreError::CorruptQuotaSnapshot {
                account_id: row.account_id_value,
                field: "stale_penalty",
            });
        }
    };

    let mut snapshot = PersistedQuotaSnapshot::new(parsed_account_id, source)
        .with_observed_unix_seconds(observed)
        .with_route_band(row.route_band, remaining)
        .with_stale_penalty(stale);
    if let Some(reset) = reset {
        snapshot = snapshot.with_reset_unix_seconds(reset);
    }
    if let Some(reset_credits) = reset_credits {
        snapshot = snapshot.with_reset_credits_available(reset_credits);
    }

    Ok(snapshot)
}

pub(super) fn selector_window_from_snapshot(
    snapshot: &PersistedQuotaSnapshot,
) -> PersistedSelectorQuotaWindow {
    let status = if snapshot.remaining_headroom() == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else if snapshot.stale_penalty() {
        SelectorQuotaWindowStatus::Stale
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    let mut window = PersistedSelectorQuotaWindow::new(
        snapshot.account_id().clone(),
        snapshot.route_band(),
        DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS,
        status,
    )
    .with_remaining_headroom(snapshot.remaining_headroom())
    .with_effective(true)
    .with_observed_unix_seconds(snapshot.observed_unix_seconds());
    if let Some(reset_unix_seconds) = snapshot.reset_unix_seconds() {
        window = window.with_reset_unix_seconds(reset_unix_seconds);
    }

    window
}
