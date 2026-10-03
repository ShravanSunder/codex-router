//! Current-schema, generation-checked credit policy writes for interactive settings.

use std::path::Path;
use std::time::Duration;

use codex_router_core::credit_usage::CreditUsagePolicy;
use codex_router_core::ids::AccountId;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;

use crate::sqlite::StateStoreError;

/// Writable credit policy store that refuses to migrate interactive state.
#[derive(Debug)]
pub struct AsyncCreditUsagePolicyMutationStore {
    pool: SqlitePool,
}

impl AsyncCreditUsagePolicyMutationStore {
    /// Opens only an existing native database with the current validated schema.
    pub async fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .create_if_missing(false)
            .busy_timeout(Duration::ZERO);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(policy_store_sqlx_error)?;

        let schema_is_current = match pool.acquire().await {
            Ok(mut connection) => {
                crate::account_migrations::validate_native_read_only_schema(&mut connection)
                    .await
                    .is_ok()
            }
            Err(_) => false,
        };
        if !schema_is_current {
            pool.close().await;
            return Err(StateStoreError::CreditUsagePolicySchemaUpgradeRequired);
        }

        Ok(Self { pool })
    }

    /// Saves and reads back a policy only if the account still has the pinned credential generation.
    pub async fn save_account_credit_usage_policy(
        &self,
        account_id: &AccountId,
        expected_credential_generation: Option<u64>,
        policy: CreditUsagePolicy,
    ) -> Result<CreditUsagePolicy, StateStoreError> {
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(policy_store_sqlx_error)?;

        let row = sqlx::query!(
            "SELECT active_credential_generation
               FROM accounts
              WHERE account_id = ?1",
            account_id.as_str(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(policy_store_sqlx_error)?;
        let Some(row) = row else {
            transaction
                .rollback()
                .await
                .map_err(policy_store_sqlx_error)?;
            return Err(StateStoreError::CreditUsagePolicyAccountUnavailable);
        };
        let actual_generation = row
            .active_credential_generation
            .map(|generation| {
                u64::try_from(generation).map_err(|_| StateStoreError::CorruptAccount {
                    account_id: account_id.as_str().to_owned(),
                    field: "active_credential_generation",
                })
            })
            .transpose()?;
        if actual_generation != expected_credential_generation {
            transaction
                .rollback()
                .await
                .map_err(policy_store_sqlx_error)?;
            return Err(StateStoreError::CreditUsagePolicyTargetChanged);
        }

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
        .execute(&mut *transaction)
        .await
        .map_err(policy_store_sqlx_error)?;

        let stored_policy = sqlx::query!(
            "SELECT allow_credits
               FROM account_credit_policies
              WHERE account_id = ?1",
            account_id.as_str(),
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(policy_store_sqlx_error)?
        .map(|row| row.allow_credits)
        .ok_or_else(|| StateStoreError::CorruptAccount {
            account_id: account_id.as_str().to_owned(),
            field: "allow_credits",
        })?;
        let stored_policy = match stored_policy {
            0 => CreditUsagePolicy::Disallow,
            1 => CreditUsagePolicy::Allow,
            _ => {
                transaction
                    .rollback()
                    .await
                    .map_err(policy_store_sqlx_error)?;
                return Err(StateStoreError::CorruptAccount {
                    account_id: account_id.as_str().to_owned(),
                    field: "allow_credits",
                });
            }
        };

        transaction
            .commit()
            .await
            .map_err(policy_store_sqlx_error)?;
        Ok(stored_policy)
    }

    /// Closes the mutation pool.
    pub async fn close(self) {
        self.pool.close().await;
    }
}

fn policy_store_sqlx_error(error: sqlx::Error) -> StateStoreError {
    if crate::sqlite::is_busy_or_locked_sqlx_error(&error) {
        StateStoreError::CreditUsagePolicyDatabaseBusy
    } else {
        crate::sqlite::sqlx_error(error)
    }
}
