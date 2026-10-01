//! Per-account credit policy persistence.

use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::ids::AccountId;

use crate::sqlite::AsyncSqliteStateStore;
use crate::sqlite::StateStoreError;

use super::corrupt_credit_account;

impl AsyncSqliteStateStore {
    /// Loads the saved per-account credit policy, defaulting absent rows to Disallow.
    pub async fn load_account_credit_usage_policy(
        &self,
        account_id: &AccountId,
    ) -> Result<CreditUsagePolicy, StateStoreError> {
        let row = sqlx::query!(
            "SELECT allow_credits
               FROM account_credit_policies
              WHERE account_id = ?1",
            account_id.as_str(),
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(crate::sqlite::sqlx_error)?;

        match row.map(|row| row.allow_credits) {
            None | Some(0) => Ok(CreditUsagePolicy::Disallow),
            Some(1) => Ok(CreditUsagePolicy::Allow),
            Some(_) => Err(corrupt_credit_account(account_id, "allow_credits")),
        }
    }

    /// Saves a fixture policy using the async SQLite pool.
    #[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
    pub async fn save_account_credit_usage_policy(
        &self,
        account_id: &AccountId,
        policy: CreditUsagePolicy,
    ) -> Result<(), StateStoreError> {
        let allow_credits = if policy.allows_credit_usage() {
            1_i64
        } else {
            0_i64
        };
        sqlx::query!(
            "INSERT INTO account_credit_policies (account_id, allow_credits)
             VALUES (?1, ?2)
             ON CONFLICT(account_id) DO UPDATE SET
               allow_credits = excluded.allow_credits",
            account_id.as_str(),
            allow_credits,
        )
        .execute(&self.pool)
        .await
        .map_err(crate::sqlite::sqlx_error)?;

        Ok(())
    }
}
