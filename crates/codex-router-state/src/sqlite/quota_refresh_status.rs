//! Refresh success metadata without replacing quota evidence or routing restrictions.

use codex_router_core::ids::AccountId;

use super::{AsyncSqliteStateStore, StateStoreError, sqlx_error, u64_to_i64};

pub(super) const REFRESH_SUCCESS_STATUS_UPSERT: &str = "INSERT INTO quota_refresh_status (
               account_id, route_band, last_success_unix_seconds,
               last_attempt_unix_seconds, last_error_class,
               stale_after_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?3, NULL, ?4)
             ON CONFLICT(account_id, route_band) DO UPDATE SET
               last_success_unix_seconds = excluded.last_success_unix_seconds,
               last_attempt_unix_seconds = excluded.last_attempt_unix_seconds,
               last_error_class = excluded.last_error_class,
               stale_after_unix_seconds = excluded.stale_after_unix_seconds";

impl AsyncSqliteStateStore {
    /// Records successful refresh metadata while preserving quota and routing state.
    pub async fn record_refresh_success_status(
        &self,
        account_id: &AccountId,
        route_band: &str,
        last_success_unix_seconds: u64,
        stale_after_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let last_success = u64_to_i64(last_success_unix_seconds)?;
        let stale_after = u64_to_i64(stale_after_unix_seconds)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(REFRESH_SUCCESS_STATUS_UPSERT)
            .bind(account_id.as_str())
            .bind(route_band)
            .bind(last_success)
            .bind(stale_after)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
        transaction.commit().await.map_err(sqlx_error)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "quota_refresh_status_tests.rs"]
mod tests;
