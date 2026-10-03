//! SQLite sync accounts responsibilities.
use super::account_store::CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS;
use super::*;

impl SqliteStateStore {
    /// Opens a SQLite state database and applies migrations.
    pub fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let connection = Connection::open(database_path).map_err(sqlite_error)?;
        let mut store = Self {
            database_path: database_path.to_path_buf(),
            connection,
        };
        store.migrate()?;
        store.ensure_async_read_only_schema()?;
        store.apply_v11_if_needed()?;
        store.apply_v12_if_needed()?;
        store.apply_v13_if_needed()?;
        store.verify_account_routing_policy_schema()?;

        Ok(store)
    }

    /// Returns the active schema version.
    pub fn schema_version(&self) -> i64 {
        self.connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sqlite_error)
            .unwrap_or(0)
    }

    /// Inserts or updates account metadata.
    pub fn upsert_account(&self, account: &AccountRecord) -> Result<(), StateStoreError> {
        let rows_affected = if self.table_has_column("accounts", "provider")? {
            self.connection
                .execute(
                    "INSERT INTO accounts (
                        account_id, label, status, active_credential_generation, provider
                     ) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(account_id) DO UPDATE SET
                       label = excluded.label,
                       status = excluded.status,
                       active_credential_generation = excluded.active_credential_generation
                     WHERE accounts.provider = excluded.provider",
                    params![
                        account.account_id().as_str(),
                        account.label(),
                        account.status().as_str(),
                        account
                            .active_credential_generation()
                            .map(u64_to_i64)
                            .transpose()?,
                        account.provider().as_str(),
                    ],
                )
                .map_err(sqlite_error)?
        } else if account.provider() == Provider::Openai {
            self.connection
                .execute(
                    "INSERT INTO accounts (account_id, label, status, active_credential_generation)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(account_id) DO UPDATE SET
                       label = excluded.label,
                       status = excluded.status,
                       active_credential_generation = excluded.active_credential_generation",
                    params![
                        account.account_id().as_str(),
                        account.label(),
                        account.status().as_str(),
                        account
                            .active_credential_generation()
                            .map(u64_to_i64)
                            .transpose()?
                    ],
                )
                .map_err(sqlite_error)?
        } else {
            return Err(StateStoreError::AccountProviderImmutable);
        };
        if rows_affected == 0 {
            return Err(StateStoreError::AccountProviderImmutable);
        }

        Ok(())
    }

    /// Loads account metadata. Corrupt rows fail closed for that account.
    pub fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError> {
        let provider_column = if self.table_has_column("accounts", "provider")? {
            "provider"
        } else {
            "'openai'"
        };
        let query = format!(
            "SELECT account_id, label, status, active_credential_generation, {provider_column}
               FROM accounts
              WHERE account_id = ?1"
        );
        let row = self
            .connection
            .query_row(&query, params![account_id.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .optional()
            .map_err(sqlite_error)?;

        let Some((
            account_id_value,
            label,
            status_value,
            active_credential_generation,
            provider_value,
        )) = row
        else {
            return Ok(None);
        };

        parse_account_row(
            account_id_value,
            label,
            status_value,
            active_credential_generation,
            provider_value,
        )
        .map(Some)
    }

    /// Lists account metadata in deterministic selector order.
    pub fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        let provider_column = if self.table_has_column("accounts", "provider")? {
            "provider"
        } else {
            "'openai'"
        };
        let query = format!(
            "SELECT account_id, label, status, active_credential_generation, {provider_column}
               FROM accounts
              ORDER BY account_id"
        );
        let mut statement = self.connection.prepare(&query).map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut accounts = Vec::new();
        for row in rows {
            let (
                account_id_value,
                label,
                status_value,
                active_credential_generation,
                provider_value,
            ) = row.map_err(sqlite_error)?;
            accounts.push(parse_account_row(
                account_id_value,
                label,
                status_value,
                active_credential_generation,
                provider_value,
            )?);
        }

        Ok(accounts)
    }

    /// Returns the next credential generation for an account.
    pub fn next_credential_generation(
        &self,
        account_id: &AccountId,
    ) -> Result<u64, StateStoreError> {
        let current_generation = self
            .load_account(account_id)?
            .and_then(|account| account.active_credential_generation())
            .unwrap_or(0);

        current_generation
            .checked_add(1)
            .ok_or_else(|| StateStoreError::Sqlite {
                message: "credential generation overflow".to_owned(),
            })
    }

    /// Activates one credential generation and invalidates quota selector state.
    pub fn activate_account_credential_generation_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let active_generation = u64_to_i64(active_credential_generation)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "UPDATE accounts
                    SET status = ?2,
                        active_credential_generation = ?3
                  WHERE account_id = ?1",
                params![account_id.as_str(), status.as_str(), active_generation],
            )
            .map_err(sqlite_error)?;
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
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    /// Activates one credential generation only if account state still matches the caller's read.
    pub fn activate_account_credential_generation_if_current_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        expected_active_credential_generation: u64,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let expected_generation = u64_to_i64(expected_active_credential_generation)?;
        let active_generation = u64_to_i64(active_credential_generation)?;
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        let updated = transaction
            .execute(
                "UPDATE accounts
                    SET status = ?2,
                        active_credential_generation = ?3
                  WHERE account_id = ?1
                    AND status = ?4
                    AND active_credential_generation = ?5",
                params![
                    account_id.as_str(),
                    status.as_str(),
                    active_generation,
                    AccountStatus::Enabled.as_str(),
                    expected_generation,
                ],
            )
            .map_err(sqlite_error)?;
        if updated != 1 {
            transaction.rollback().map_err(sqlite_error)?;
            return Err(StateStoreError::AccountConcurrentModification {
                account_id: account_id.as_str().to_owned(),
            });
        }
        invalidate_credential_mutation_quota_sync(&transaction, account_id)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    /// Inserts raw account metadata for corruption fixtures.
    #[cfg(test)]
    pub fn insert_raw_account_for_test(
        &self,
        account_id: &str,
        label: &str,
        status: &str,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO accounts (account_id, label, status) VALUES (?1, ?2, ?3)",
                params![account_id, label, status],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
impl AccountStateRepository for SqliteStateStore {
    fn upsert_account(&self, account: &AccountRecord) -> Result<(), StateStoreError> {
        self.upsert_account(account)
    }

    fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError> {
        self.load_account(account_id)
    }

    fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        self.list_accounts()
    }
}
