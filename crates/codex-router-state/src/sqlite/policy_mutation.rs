//! SQLite policy mutation responsibilities.
use super::*;
const WEEKLY_FLOOR_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
];

const WEEKLY_FLOOR_MUTATION_DEADLINE: Duration = Duration::from_millis(250);

#[derive(Debug)]
enum WeeklyQuotaFloorMutationAttemptError {
    AccountNotFound,
    AccountLabelAmbiguous,
    InvalidAccountMetadata,
    Sqlite(sqlx::Error),
}

#[derive(Clone, Copy, Debug)]
enum WeeklyQuotaFloorAccountSelector<'a> {
    AccountId(&'a AccountId),
    AccountLabel(&'a str),
}

impl From<sqlx::Error> for WeeklyQuotaFloorMutationAttemptError {
    fn from(error: sqlx::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl AsyncWeeklyQuotaFloorMutationStore {
    /// Opens an existing current-schema database without migrating or checkpointing.
    pub async fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(0));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(|_| redacted_weekly_floor_sqlite_error("unable to open database"))?;
        let authority = {
            let mut connection = pool
                .acquire()
                .await
                .map_err(|_| redacted_weekly_floor_sqlite_error("unable to verify schema"))?;
            crate::account_migrations::migration_authority(&mut connection).await
        };
        match authority {
            Ok(crate::account_migrations::MigrationAuthority::Legacy) => {}
            Ok(crate::account_migrations::MigrationAuthority::NativeCurrent) => {
                let native_schema_valid = {
                    let mut connection = pool.acquire().await.map_err(|_| {
                        redacted_weekly_floor_sqlite_error("unable to verify schema")
                    })?;
                    crate::account_migrations::validate_native_read_only_schema(&mut connection)
                        .await
                        .is_ok()
                };
                if !native_schema_valid {
                    pool.close().await;
                    return Err(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired);
                }
            }
            Ok(crate::account_migrations::MigrationAuthority::NativeUpgradeRequired) | Err(_) => {
                pool.close().await;
                return Err(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired);
            }
        }
        let version = sqlx::query("PRAGMA user_version")
            .fetch_one(&pool)
            .await
            .map(|row| row.get::<i64, _>(0))
            .map_err(|_| redacted_weekly_floor_sqlite_error("unable to read schema version"))?;
        if version != CURRENT_SCHEMA_VERSION {
            pool.close().await;
            return Err(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired);
        }
        let table_exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = 'account_routing_policies'
            )",
        )
        .fetch_one(&pool)
        .await
        .map_err(|_| redacted_weekly_floor_sqlite_error("unable to verify policy schema"))?;
        if table_exists != 1 {
            pool.close().await;
            return Err(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired);
        }
        let columns = sqlx::query("PRAGMA table_info(account_routing_policies)")
            .fetch_all(&pool)
            .await
            .map_err(|_| redacted_weekly_floor_sqlite_error("unable to verify policy schema"))?
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        if !["account_id", "weekly_quota_floor_basis_points"]
            .iter()
            .all(|required| columns.iter().any(|column| column == required))
        {
            pool.close().await;
            return Err(StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired);
        }

        Ok(Self { pool })
    }

    /// Atomically enables or disables one account's weekly quota floor.
    pub async fn set_weekly_quota_floor_by_label(
        &self,
        account_label: &str,
        floor: Option<WeeklyQuotaFloorBasisPoints>,
    ) -> Result<WeeklyQuotaFloorMutationResult, StateStoreError> {
        self.set_weekly_quota_floor(
            WeeklyQuotaFloorAccountSelector::AccountLabel(account_label),
            floor,
        )
        .await
    }

    /// Atomically enables or disables one account's weekly quota floor by stable account ID.
    pub async fn set_weekly_quota_floor_by_account_id(
        &self,
        account_id: &AccountId,
        floor: Option<WeeklyQuotaFloorBasisPoints>,
    ) -> Result<WeeklyQuotaFloorMutationResult, StateStoreError> {
        self.set_weekly_quota_floor(
            WeeklyQuotaFloorAccountSelector::AccountId(account_id),
            floor,
        )
        .await
    }

    /// Sets one account's lifecycle status by its exact display label.
    pub async fn set_account_status_by_label(
        &self,
        account_label: &str,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let deadline = Instant::now() + WEEKLY_FLOOR_MUTATION_DEADLINE;
        let mut next_delay_index = 0;
        loop {
            if deadline.checked_duration_since(Instant::now()).is_none() {
                return Err(StateStoreError::AccountStatusDatabaseBusy);
            }
            match self
                .try_set_account_status_by_label(account_label, status)
                .await
            {
                Ok(()) => return Ok(()),
                Err(WeeklyQuotaFloorMutationAttemptError::Sqlite(error))
                    if is_busy_or_locked_sqlx_error(&error) =>
                {
                    let Some(delay) = WEEKLY_FLOOR_RETRY_DELAYS.get(next_delay_index).copied()
                    else {
                        return Err(StateStoreError::AccountStatusDatabaseBusy);
                    };
                    next_delay_index += 1;
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return Err(StateStoreError::AccountStatusDatabaseBusy);
                    };
                    if delay >= remaining {
                        return Err(StateStoreError::AccountStatusDatabaseBusy);
                    }
                    tokio::time::sleep_until(tokio::time::Instant::from_std(
                        Instant::now() + delay,
                    ))
                    .await;
                }
                Err(WeeklyQuotaFloorMutationAttemptError::AccountNotFound) => {
                    return Err(StateStoreError::AccountStatusAccountNotFound);
                }
                Err(WeeklyQuotaFloorMutationAttemptError::AccountLabelAmbiguous) => {
                    return Err(StateStoreError::AccountStatusAccountLabelAmbiguous);
                }
                Err(WeeklyQuotaFloorMutationAttemptError::InvalidAccountMetadata)
                | Err(WeeklyQuotaFloorMutationAttemptError::Sqlite(_)) => {
                    return Err(StateStoreError::AccountStatusSchemaUpgradeRequired);
                }
            }
        }
    }

    async fn set_weekly_quota_floor(
        &self,
        account_selector: WeeklyQuotaFloorAccountSelector<'_>,
        floor: Option<WeeklyQuotaFloorBasisPoints>,
    ) -> Result<WeeklyQuotaFloorMutationResult, StateStoreError> {
        let deadline = Instant::now() + WEEKLY_FLOOR_MUTATION_DEADLINE;
        let mut next_delay_index = 0;
        loop {
            if deadline.checked_duration_since(Instant::now()).is_none() {
                return Err(StateStoreError::WeeklyQuotaFloorDatabaseBusy);
            }
            let attempt = self
                .try_set_weekly_quota_floor(account_selector, floor)
                .await;
            match attempt {
                Ok(result) => return Ok(result),
                Err(WeeklyQuotaFloorMutationAttemptError::Sqlite(error))
                    if is_busy_or_locked_sqlx_error(&error) =>
                {
                    let Some(delay) = WEEKLY_FLOOR_RETRY_DELAYS.get(next_delay_index).copied()
                    else {
                        return Err(StateStoreError::WeeklyQuotaFloorDatabaseBusy);
                    };
                    next_delay_index += 1;
                    let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                        return Err(StateStoreError::WeeklyQuotaFloorDatabaseBusy);
                    };
                    if delay >= remaining {
                        return Err(StateStoreError::WeeklyQuotaFloorDatabaseBusy);
                    }
                    tokio::time::sleep_until(tokio::time::Instant::from_std(
                        Instant::now() + delay,
                    ))
                    .await;
                }
                Err(WeeklyQuotaFloorMutationAttemptError::AccountNotFound) => {
                    return Err(StateStoreError::WeeklyQuotaFloorAccountNotFound);
                }
                Err(WeeklyQuotaFloorMutationAttemptError::AccountLabelAmbiguous) => {
                    return Err(StateStoreError::WeeklyQuotaFloorAccountLabelAmbiguous);
                }
                Err(WeeklyQuotaFloorMutationAttemptError::InvalidAccountMetadata) => {
                    return Err(redacted_weekly_floor_sqlite_error(
                        "invalid account metadata",
                    ));
                }
                Err(WeeklyQuotaFloorMutationAttemptError::Sqlite(_)) => {
                    return Err(redacted_weekly_floor_sqlite_error("mutation failed"));
                }
            }
        }
    }

    async fn try_set_weekly_quota_floor(
        &self,
        account_selector: WeeklyQuotaFloorAccountSelector<'_>,
        floor: Option<WeeklyQuotaFloorBasisPoints>,
    ) -> Result<WeeklyQuotaFloorMutationResult, WeeklyQuotaFloorMutationAttemptError> {
        let mut transaction = self.pool.begin().await?;
        let account_id = match account_selector {
            WeeklyQuotaFloorAccountSelector::AccountId(account_id) => {
                let account_exists = sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE account_id = ?1)",
                )
                .bind(account_id.as_str())
                .fetch_one(&mut *transaction)
                .await?;
                if !account_exists {
                    transaction.rollback().await?;
                    return Err(WeeklyQuotaFloorMutationAttemptError::AccountNotFound);
                }
                account_id.clone()
            }
            WeeklyQuotaFloorAccountSelector::AccountLabel(account_label) => {
                let matching_account_ids = sqlx::query_scalar::<_, String>(
                    "SELECT account_id FROM accounts WHERE label = ?1 ORDER BY account_id LIMIT 2",
                )
                .bind(account_label)
                .fetch_all(&mut *transaction)
                .await?;
                let account_id_value = match matching_account_ids.as_slice() {
                    [] => {
                        transaction.rollback().await?;
                        return Err(WeeklyQuotaFloorMutationAttemptError::AccountNotFound);
                    }
                    [account_id] => account_id,
                    [_, _, ..] => {
                        transaction.rollback().await?;
                        return Err(WeeklyQuotaFloorMutationAttemptError::AccountLabelAmbiguous);
                    }
                };
                match AccountId::new(account_id_value.clone()) {
                    Ok(account_id) => account_id,
                    Err(_) => {
                        transaction.rollback().await?;
                        return Err(WeeklyQuotaFloorMutationAttemptError::InvalidAccountMetadata);
                    }
                }
            }
        };
        let result = if let Some(floor) = floor {
            sqlx::query(
                "INSERT INTO account_routing_policies (
                    account_id, weekly_quota_floor_basis_points
                 ) VALUES (?1, ?2)
                 ON CONFLICT(account_id) DO UPDATE SET
                    weekly_quota_floor_basis_points = excluded.weekly_quota_floor_basis_points",
            )
            .bind(account_id.as_str())
            .bind(i64::from(floor.basis_points()))
            .execute(&mut *transaction)
            .await?;
            WeeklyQuotaFloorMutationResult::Enabled(AccountRoutingPolicy::new(account_id, floor))
        } else {
            sqlx::query("DELETE FROM account_routing_policies WHERE account_id = ?1")
                .bind(account_id.as_str())
                .execute(&mut *transaction)
                .await?;
            WeeklyQuotaFloorMutationResult::Disabled
        };
        transaction.commit().await?;
        Ok(result)
    }

    async fn try_set_account_status_by_label(
        &self,
        account_label: &str,
        status: AccountStatus,
    ) -> Result<(), WeeklyQuotaFloorMutationAttemptError> {
        let mut transaction = self.pool.begin().await?;
        let matching_account_ids = sqlx::query_scalar::<_, String>(
            "SELECT account_id FROM accounts WHERE label = ?1 ORDER BY account_id LIMIT 2",
        )
        .bind(account_label)
        .fetch_all(&mut *transaction)
        .await?;
        let account_id = match matching_account_ids.as_slice() {
            [] => {
                transaction.rollback().await?;
                return Err(WeeklyQuotaFloorMutationAttemptError::AccountNotFound);
            }
            [account_id] => account_id,
            [_, _, ..] => {
                transaction.rollback().await?;
                return Err(WeeklyQuotaFloorMutationAttemptError::AccountLabelAmbiguous);
            }
        };
        sqlx::query("UPDATE accounts SET status = ?2 WHERE account_id = ?1")
            .bind(account_id)
            .bind(status.as_str())
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;

        Ok(())
    }

    /// Closes the mutation pool without issuing a WAL checkpoint.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

pub(super) fn redacted_weekly_floor_sqlite_error(message: &'static str) -> StateStoreError {
    StateStoreError::Sqlite {
        message: message.to_owned(),
    }
}
