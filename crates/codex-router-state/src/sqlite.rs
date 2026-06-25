//! SQLite metadata store.

use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;
use codex_router_core::routes::RouteBand;
use futures_util::future::BoxFuture;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqlitePoolOptions;
use thiserror::Error;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::affinity_owner::AffinitySourceTransport;
use crate::affinity_owner::PreviousResponseAffinityOwnerLookup;
use crate::affinity_owner::PreviousResponseAffinityOwnerRecord;
use crate::quota_snapshot::PersistedQuotaHistoryObservation;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::QuotaHistoryRefreshOutcome;
use crate::quota_snapshot::QuotaRefreshErrorClass;
use crate::quota_snapshot::QuotaRefreshStatusView;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::repositories::AccountStateRepository;
use crate::repositories::AffinityRepository;
use crate::repositories::QuotaSnapshotRepository;
use crate::repositories::SelectorQuotaRepository;

const CURRENT_SCHEMA_VERSION: i64 = 7;
const DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS: u64 = 18_000;
const CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS: &[&str] = &[
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
    "code_review",
];
const SELECTOR_INVALIDATED_ROUTE_BANDS: &[&str] = &[
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
];
const ASYNC_V1_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS accounts (
        account_id TEXT PRIMARY KEY NOT NULL,
        label TEXT NOT NULL,
        status TEXT NOT NULL,
        active_credential_generation INTEGER
    )",
    "CREATE TABLE IF NOT EXISTS quota_snapshots (
        account_id TEXT NOT NULL,
        source TEXT NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        route_band TEXT NOT NULL,
        remaining_headroom INTEGER NOT NULL,
        reset_unix_seconds INTEGER,
        reset_credits_available INTEGER,
        stale_penalty INTEGER NOT NULL,
        PRIMARY KEY (account_id, route_band)
    )",
    "CREATE TABLE IF NOT EXISTS affinity_pins (
        affinity_key TEXT PRIMARY KEY NOT NULL,
        account_id TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS selector_quota_windows (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        limit_window_seconds INTEGER NOT NULL,
        status TEXT NOT NULL,
        remaining_headroom INTEGER NOT NULL,
        reset_unix_seconds INTEGER,
        effective INTEGER NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        PRIMARY KEY (account_id, route_band, limit_window_seconds)
    )",
    "CREATE TABLE IF NOT EXISTS quota_refresh_status (
        account_id TEXT NOT NULL,
        route_band TEXT NOT NULL,
        last_success_unix_seconds INTEGER,
        last_attempt_unix_seconds INTEGER,
        last_error_class TEXT,
        stale_after_unix_seconds INTEGER,
        PRIMARY KEY (account_id, route_band)
    )",
    "CREATE TABLE IF NOT EXISTS previous_response_affinity_owners (
        affinity_key_hash TEXT NOT NULL,
        route_band TEXT NOT NULL,
        account_id TEXT NOT NULL,
        credential_generation INTEGER NOT NULL,
        source_transport TEXT NOT NULL,
        created_unix_seconds INTEGER NOT NULL,
        PRIMARY KEY (affinity_key_hash, route_band, account_id)
    )",
    "PRAGMA user_version = 7",
];
const ASYNC_QUOTA_HISTORY_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS quota_history_observations (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        account_id TEXT NOT NULL,
        account_label TEXT NOT NULL,
        route_band TEXT NOT NULL,
        limit_window_seconds INTEGER NOT NULL,
        observed_unix_seconds INTEGER NOT NULL,
        remaining_headroom INTEGER NOT NULL,
        reset_unix_seconds INTEGER,
        window_status TEXT NOT NULL,
        effective INTEGER NOT NULL,
        refresh_source TEXT NOT NULL,
        refresh_success INTEGER NOT NULL,
        refresh_error_class TEXT,
        reset_credits_available INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS quota_history_window_lookup
        ON quota_history_observations (
            account_id, route_band, limit_window_seconds, observed_unix_seconds
        )",
];

/// SQLite state store failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StateStoreError {
    /// SQLite failed.
    #[error("sqlite state store failed: {message}")]
    Sqlite {
        /// Redacted SQLite error message.
        message: String,
    },
    /// Database schema is newer or otherwise unsupported.
    #[error("unsupported sqlite schema version: {version}")]
    UnsupportedSchemaVersion {
        /// Observed schema version.
        version: i64,
    },
    /// Account metadata is corrupt; affected account fails closed.
    #[error("corrupt account metadata for {account_id}: {field}")]
    CorruptAccount {
        /// Affected account id.
        account_id: String,
        /// Corrupt field name.
        field: &'static str,
    },
    /// Quota snapshot metadata is corrupt; affected snapshot fails closed.
    #[error("corrupt quota snapshot metadata for {account_id}: {field}")]
    CorruptQuotaSnapshot {
        /// Affected account id.
        account_id: String,
        /// Corrupt field name.
        field: &'static str,
    },
    /// Account state changed while a credential refresh was being committed.
    #[error("account state changed during credential commit for {account_id}")]
    AccountConcurrentModification {
        /// Affected account id.
        account_id: String,
    },
}

/// SQLite-backed metadata repository.
pub struct SqliteStateStore {
    database_path: PathBuf,
    connection: Connection,
}

impl fmt::Debug for SqliteStateStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SqliteStateStore")
            .field("database_path", &self.database_path)
            .finish_non_exhaustive()
    }
}

/// Async SQLite state store backed by SQLx.
#[derive(Clone, Debug)]
pub struct AsyncSqliteStateStore {
    database_path: PathBuf,
    pool: sqlx::SqlitePool,
}

impl AsyncSqliteStateStore {
    /// Opens a SQLite state database through SQLx and applies supported migrations.
    pub async fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(sqlx_error)?;
        let store = Self {
            database_path: database_path.to_path_buf(),
            pool,
        };
        store.migrate().await?;
        store.ensure_quota_history_schema().await?;

        Ok(store)
    }

    /// Returns the database path used by this async store.
    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Returns the active schema version.
    pub async fn schema_version(&self) -> Result<i64, StateStoreError> {
        sqlx::query("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await
            .map(|row| row.get::<i64, _>(0))
            .map_err(sqlx_error)
    }

    /// Loads selector input rows for one route band.
    pub async fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
        now_unix_seconds: u64,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        let accounts = self.list_accounts().await?;
        let mut inputs = Vec::new();
        for account in accounts {
            let mut windows = self
                .load_selector_windows(account.account_id(), route_band)
                .await?;
            let refresh_status = self
                .load_quota_refresh_status(account.account_id(), route_band)
                .await?;
            if selector_windows_are_stale(&windows, refresh_status.as_ref(), now_unix_seconds) {
                mark_selector_windows_stale(&mut windows);
            }
            inputs.push(SelectorQuotaInput::new(
                account.account_id().clone(),
                account.label(),
                account.status(),
                account.active_credential_generation(),
                route_band,
                windows,
            ));
        }

        Ok(inputs)
    }

    /// Marks an account's route-band quota as exhausted for future selector reads.
    pub async fn mark_route_band_quota_exhausted(
        &self,
        account_id: &AccountId,
        route_band: &str,
        observed_unix_seconds: u64,
    ) -> Result<(), StateStoreError> {
        let observed_unix_seconds = u64_to_i64(observed_unix_seconds)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
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
        let update = sqlx::query(
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
        if update.rows_affected() == 0 && selector_route_band(route_band) {
            sqlx::query(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, ?5)",
            )
            .bind(account_id.as_str())
            .bind(route_band)
            .bind(u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?)
            .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
            .bind(observed_unix_seconds)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
        }
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    async fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        let rows = sqlx::query(
            "SELECT account_id, label, status, active_credential_generation
               FROM accounts
              ORDER BY account_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut accounts = Vec::new();
        for row in rows {
            accounts.push(parse_account_row(
                row.get::<String, _>(0),
                row.get::<String, _>(1),
                row.get::<String, _>(2),
                row.get::<Option<i64>, _>(3),
            )?);
        }

        Ok(accounts)
    }

    /// Loads account metadata through the async state connection pool.
    pub async fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError> {
        let row = sqlx::query(
            "SELECT account_id, label, status, active_credential_generation
               FROM accounts
              WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        parse_account_row(
            row.get::<String, _>(0),
            row.get::<String, _>(1),
            row.get::<String, _>(2),
            row.get::<Option<i64>, _>(3),
        )
        .map(Some)
    }

    /// Returns the next credential generation through the async state pool.
    pub async fn next_credential_generation(
        &self,
        account_id: &AccountId,
    ) -> Result<u64, StateStoreError> {
        let current_generation = self
            .load_account(account_id)
            .await?
            .and_then(|account| account.active_credential_generation())
            .unwrap_or(0);

        current_generation
            .checked_add(1)
            .ok_or_else(|| StateStoreError::Sqlite {
                message: "credential generation overflow".to_owned(),
            })
    }

    /// Activates one credential generation and invalidates quota selector state.
    pub async fn activate_account_credential_generation_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let active_generation = u64_to_i64(active_credential_generation)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        sqlx::query(
            "UPDATE accounts
                SET status = ?2,
                    active_credential_generation = ?3
              WHERE account_id = ?1",
        )
        .bind(account_id.as_str())
        .bind(status.as_str())
        .bind(active_generation)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        for route_band in CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS {
            sqlx::query(
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
            )
            .bind(account_id.as_str())
            .bind(QuotaSnapshotSource::CredentialMutation.as_str())
            .bind(*route_band)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
            sqlx::query(
                "DELETE FROM selector_quota_windows
                  WHERE account_id = ?1 AND route_band = ?2",
            )
            .bind(account_id.as_str())
            .bind(*route_band)
            .execute(&mut *transaction)
            .await
            .map_err(sqlx_error)?;
            if selector_route_band(route_band) {
                sqlx::query(
                    "INSERT INTO selector_quota_windows (
                       account_id, route_band, limit_window_seconds, status,
                       remaining_headroom, reset_unix_seconds, effective,
                       observed_unix_seconds
                     )
                     VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, 0)",
                )
                .bind(account_id.as_str())
                .bind(*route_band)
                .bind(u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?)
                .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(sqlx_error)?;
            }
        }
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Activates one credential generation only if account state still matches the caller's read.
    pub async fn activate_account_credential_generation_if_current_and_invalidate_quota(
        &self,
        account_id: &AccountId,
        expected_active_credential_generation: u64,
        active_credential_generation: u64,
        status: AccountStatus,
    ) -> Result<(), StateStoreError> {
        let expected_generation = u64_to_i64(expected_active_credential_generation)?;
        let active_generation = u64_to_i64(active_credential_generation)?;
        let mut transaction = self.pool.begin().await.map_err(sqlx_error)?;
        let update = sqlx::query(
            "UPDATE accounts
                SET status = ?2,
                    active_credential_generation = ?3
              WHERE account_id = ?1
                AND status = ?4
                AND active_credential_generation = ?5",
        )
        .bind(account_id.as_str())
        .bind(status.as_str())
        .bind(active_generation)
        .bind(AccountStatus::Enabled.as_str())
        .bind(expected_generation)
        .execute(&mut *transaction)
        .await
        .map_err(sqlx_error)?;
        if update.rows_affected() != 1 {
            transaction.rollback().await.map_err(sqlx_error)?;
            return Err(StateStoreError::AccountConcurrentModification {
                account_id: account_id.as_str().to_owned(),
            });
        }
        invalidate_credential_mutation_quota_async(&mut transaction, account_id).await?;
        transaction.commit().await.map_err(sqlx_error)?;

        Ok(())
    }

    /// Writes a previous-response owner record through the async state pool.
    pub async fn write_previous_response_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), StateStoreError> {
        sqlx::query(
            "INSERT INTO previous_response_affinity_owners (
               affinity_key_hash, route_band, account_id, credential_generation,
               source_transport, created_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(affinity_key_hash, route_band, account_id) DO UPDATE SET
               credential_generation = excluded.credential_generation,
               source_transport = excluded.source_transport,
               created_unix_seconds = excluded.created_unix_seconds",
        )
        .bind(owner.affinity_key_hash().as_str())
        .bind(owner.route_band().as_str())
        .bind(owner.account_id().as_str())
        .bind(u64_to_i64(owner.credential_generation())?)
        .bind(owner.source_transport().as_str())
        .bind(u64_to_i64(owner.created_unix_seconds())?)
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

        Ok(())
    }

    async fn load_quota_refresh_status(
        &self,
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
        .fetch_optional(&self.pool)
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

    async fn load_selector_windows(
        &self,
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
        .fetch_all(&self.pool)
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

    /// Loads a hashed previous-response owner record for one route band.
    pub async fn load_previous_response_owner(
        &self,
        affinity_key_hash: &AffinityKeyHash,
        route_band: &str,
    ) -> Result<PreviousResponseAffinityOwnerLookup, StateStoreError> {
        let rows = sqlx::query(
            "SELECT affinity_key_hash, route_band, account_id, credential_generation,
                    source_transport, created_unix_seconds
               FROM previous_response_affinity_owners
              WHERE affinity_key_hash = ?1 AND route_band = ?2
              ORDER BY account_id
              LIMIT 2",
        )
        .bind(affinity_key_hash.as_str())
        .bind(route_band)
        .fetch_all(&self.pool)
        .await
        .map_err(sqlx_error)?;

        let mut owners = Vec::new();
        for row in rows {
            owners.push(parse_previous_response_owner_row(
                row.get::<String, _>(0),
                row.get::<String, _>(1),
                row.get::<String, _>(2),
                row.get::<i64, _>(3),
                row.get::<String, _>(4),
                row.get::<i64, _>(5),
            )?);
        }

        match owners.len() {
            0 => Ok(PreviousResponseAffinityOwnerLookup::Missing),
            1 => Ok(PreviousResponseAffinityOwnerLookup::Found(owners.remove(0))),
            _ => Ok(PreviousResponseAffinityOwnerLookup::Ambiguous),
        }
    }

    /// Appends one quota history observation through the async state pool.
    pub async fn append_quota_history_observation(
        &self,
        observation: &PersistedQuotaHistoryObservation,
    ) -> Result<(), StateStoreError> {
        let (refresh_success, refresh_error_class) = match observation.refresh_outcome() {
            QuotaHistoryRefreshOutcome::Success => (1_i64, None),
            QuotaHistoryRefreshOutcome::Failure { error_class } => {
                (0_i64, Some(error_class.as_str()))
            }
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
        .execute(&self.pool)
        .await
        .map_err(sqlx_error)?;

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

    async fn migrate(&self) -> Result<(), StateStoreError> {
        match self.schema_version().await? {
            0 => self.apply_v1().await,
            CURRENT_SCHEMA_VERSION => Ok(()),
            version => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    async fn apply_v1(&self) -> Result<(), StateStoreError> {
        for statement in ASYNC_V1_SCHEMA_STATEMENTS {
            sqlx::query(*statement)
                .execute(&self.pool)
                .await
                .map_err(sqlx_error)?;
        }

        Ok(())
    }

    async fn ensure_quota_history_schema(&self) -> Result<(), StateStoreError> {
        for statement in ASYNC_QUOTA_HISTORY_SCHEMA_STATEMENTS {
            sqlx::query(*statement)
                .execute(&self.pool)
                .await
                .map_err(sqlx_error)?;
        }

        Ok(())
    }
}

/// Async quota history repository contract for Tokio runtime callers.
pub trait AsyncQuotaHistoryRepository {
    /// Appends one quota history observation.
    fn append_quota_history_observation<'a>(
        &'a self,
        observation: &'a PersistedQuotaHistoryObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;

    /// Loads quota history observations for one account/route/window/time range.
    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>>;

    /// Purges quota history older than the given observation timestamp.
    fn purge_quota_history_before(
        &self,
        observed_before_unix_seconds: u64,
    ) -> BoxFuture<'_, Result<(), StateStoreError>>;
}

impl AsyncQuotaHistoryRepository for AsyncSqliteStateStore {
    fn append_quota_history_observation<'a>(
        &'a self,
        observation: &'a PersistedQuotaHistoryObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move { self.append_quota_history_observation(observation).await })
    }

    fn quota_history_observations_for_window<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        limit_window_seconds: u64,
        observed_from_unix_seconds: u64,
        observed_to_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<PersistedQuotaHistoryObservation>, StateStoreError>> {
        Box::pin(async move {
            self.quota_history_observations_for_window(
                account_id,
                route_band,
                limit_window_seconds,
                observed_from_unix_seconds,
                observed_to_unix_seconds,
            )
            .await
        })
    }

    fn purge_quota_history_before(
        &self,
        observed_before_unix_seconds: u64,
    ) -> BoxFuture<'_, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.purge_quota_history_before(observed_before_unix_seconds)
                .await
        })
    }
}

/// Async selector quota repository contract for Tokio runtime callers.
pub trait AsyncSelectorQuotaRepository {
    /// Loads selector input rows for one route band.
    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>>;
}

impl AsyncSelectorQuotaRepository for AsyncSqliteStateStore {
    fn selector_inputs_for_route_band<'a>(
        &'a self,
        route_band: &'a str,
        now_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<Vec<SelectorQuotaInput>, StateStoreError>> {
        Box::pin(async move {
            self.selector_inputs_for_route_band(route_band, now_unix_seconds)
                .await
        })
    }
}

/// Async quota exhaustion writer for Tokio runtime callers.
pub trait AsyncQuotaExhaustionRepository {
    /// Marks an account exhausted for one route band.
    fn mark_route_band_quota_exhausted<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;
}

impl AsyncQuotaExhaustionRepository for AsyncSqliteStateStore {
    fn mark_route_band_quota_exhausted<'a>(
        &'a self,
        account_id: &'a AccountId,
        route_band: &'a str,
        observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move {
            self.mark_route_band_quota_exhausted(account_id, route_band, observed_unix_seconds)
                .await
        })
    }
}

/// Async affinity repository contract for Tokio runtime callers.
pub trait AsyncAffinityRepository {
    /// Writes a hashed previous-response owner record.
    fn write_previous_response_owner<'a>(
        &'a self,
        owner: &'a PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), StateStoreError>>;

    /// Loads a hashed previous-response owner record for one route band.
    fn load_previous_response_owner<'a>(
        &'a self,
        affinity_key_hash: &'a AffinityKeyHash,
        route_band: &'a str,
    ) -> BoxFuture<'a, Result<PreviousResponseAffinityOwnerLookup, StateStoreError>>;
}

impl AsyncAffinityRepository for AsyncSqliteStateStore {
    fn write_previous_response_owner<'a>(
        &'a self,
        owner: &'a PreviousResponseAffinityOwnerRecord,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        Box::pin(async move { self.write_previous_response_owner(owner).await })
    }

    fn load_previous_response_owner<'a>(
        &'a self,
        affinity_key_hash: &'a AffinityKeyHash,
        route_band: &'a str,
    ) -> BoxFuture<'a, Result<PreviousResponseAffinityOwnerLookup, StateStoreError>> {
        Box::pin(async move {
            self.load_previous_response_owner(affinity_key_hash, route_band)
                .await
        })
    }
}

impl SqliteStateStore {
    /// Opens a SQLite state database and applies migrations.
    pub fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let connection = Connection::open(database_path).map_err(sqlite_error)?;
        let store = Self {
            database_path: database_path.to_path_buf(),
            connection,
        };
        store.migrate()?;

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
            .map_err(sqlite_error)?;

        Ok(())
    }

    /// Loads account metadata. Corrupt rows fail closed for that account.
    pub fn load_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountRecord>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT account_id, label, status, active_credential_generation
                   FROM accounts
                  WHERE account_id = ?1",
                params![account_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some((account_id_value, label, status_value, active_credential_generation)) = row
        else {
            return Ok(None);
        };

        parse_account_row(
            account_id_value,
            label,
            status_value,
            active_credential_generation,
        )
        .map(Some)
    }

    /// Lists account metadata in deterministic selector order.
    pub fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT account_id, label, status, active_credential_generation
                   FROM accounts
                  ORDER BY account_id",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut accounts = Vec::new();
        for row in rows {
            let (account_id_value, label, status_value, active_credential_generation) =
                row.map_err(sqlite_error)?;
            accounts.push(parse_account_row(
                account_id_value,
                label,
                status_value,
                active_credential_generation,
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

    fn migrate(&self) -> Result<(), StateStoreError> {
        let version = self.schema_version();
        match version {
            0 => self.apply_v1(),
            1 => {
                self.apply_v2()?;
                self.apply_v3()?;
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()
            }
            2 => {
                self.apply_v3()?;
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()
            }
            3 => {
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()
            }
            4 => {
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()
            }
            5 => {
                self.apply_v6()?;
                self.apply_v7()
            }
            6 => self.apply_v7(),
            CURRENT_SCHEMA_VERSION => Ok(()),
            _ => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    fn apply_v1(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS accounts (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    active_credential_generation INTEGER
                );

                CREATE TABLE IF NOT EXISTS quota_snapshots (
                    account_id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    observed_unix_seconds INTEGER NOT NULL,
                    route_band TEXT NOT NULL,
                    remaining_headroom INTEGER NOT NULL,
                    reset_unix_seconds INTEGER,
                    reset_credits_available INTEGER,
                    stale_penalty INTEGER NOT NULL,
                    PRIMARY KEY (account_id, route_band)
                );

                CREATE TABLE IF NOT EXISTS affinity_pins (
                    affinity_key TEXT PRIMARY KEY NOT NULL,
                    account_id TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS selector_quota_windows (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    limit_window_seconds INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    remaining_headroom INTEGER NOT NULL,
                    reset_unix_seconds INTEGER,
                    effective INTEGER NOT NULL,
                    observed_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (account_id, route_band, limit_window_seconds)
                );

                CREATE TABLE IF NOT EXISTS quota_refresh_status (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    last_success_unix_seconds INTEGER,
                    last_attempt_unix_seconds INTEGER,
                    last_error_class TEXT,
                    stale_after_unix_seconds INTEGER,
                    PRIMARY KEY (account_id, route_band)
                );

                CREATE TABLE IF NOT EXISTS previous_response_affinity_owners (
                    affinity_key_hash TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    account_id TEXT NOT NULL,
                    credential_generation INTEGER NOT NULL,
                    source_transport TEXT NOT NULL,
                    created_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (affinity_key_hash, route_band, account_id)
                );

                PRAGMA user_version = 7;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v2(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                ALTER TABLE accounts ADD COLUMN active_credential_generation INTEGER;
                PRAGMA user_version = 2;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v3(&self) -> Result<(), StateStoreError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS selector_quota_windows (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    limit_window_seconds INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    remaining_headroom INTEGER NOT NULL,
                    reset_unix_seconds INTEGER,
                    effective INTEGER NOT NULL,
                    observed_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (account_id, route_band, limit_window_seconds)
                );
                ",
            )
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 )
                 SELECT
                   account_id,
                   route_band,
                   ?1,
                   CASE
                     WHEN remaining_headroom <= 0 THEN ?2
                     WHEN stale_penalty = 1 THEN ?3
                     ELSE ?4
                   END,
                   remaining_headroom,
                   reset_unix_seconds,
                   1,
                   observed_unix_seconds
                 FROM quota_snapshots
                 WHERE route_band IN (?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, route_band, limit_window_seconds) DO UPDATE SET
                   status = excluded.status,
                   remaining_headroom = excluded.remaining_headroom,
                   reset_unix_seconds = excluded.reset_unix_seconds,
                   effective = excluded.effective,
                   observed_unix_seconds = excluded.observed_unix_seconds",
                params![
                    u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?,
                    SelectorQuotaWindowStatus::Ineligible.as_str(),
                    SelectorQuotaWindowStatus::Stale.as_str(),
                    SelectorQuotaWindowStatus::Eligible.as_str(),
                    SELECTOR_INVALIDATED_ROUTE_BANDS[0],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[1],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[2],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[3],
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute_batch("PRAGMA user_version = 3;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v4(&self) -> Result<(), StateStoreError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(sqlite_error)?;
        transaction
            .execute(
                "DELETE FROM selector_quota_windows
                  WHERE route_band NOT IN (?1, ?2, ?3, ?4)",
                params![
                    SELECTOR_INVALIDATED_ROUTE_BANDS[0],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[1],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[2],
                    SELECTOR_INVALIDATED_ROUTE_BANDS[3],
                ],
            )
            .map_err(sqlite_error)?;
        transaction
            .execute_batch("PRAGMA user_version = 4;")
            .map_err(sqlite_error)?;
        transaction.commit().map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v5(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS quota_refresh_status (
                    account_id TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    last_success_unix_seconds INTEGER,
                    last_attempt_unix_seconds INTEGER,
                    last_error_class TEXT,
                    stale_after_unix_seconds INTEGER,
                    PRIMARY KEY (account_id, route_band)
                );

                PRAGMA user_version = 5;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v6(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS previous_response_affinity_owners (
                    affinity_key_hash TEXT NOT NULL,
                    route_band TEXT NOT NULL,
                    account_id TEXT NOT NULL,
                    credential_generation INTEGER NOT NULL,
                    source_transport TEXT NOT NULL,
                    created_unix_seconds INTEGER NOT NULL,
                    PRIMARY KEY (affinity_key_hash, route_band, account_id)
                );

                PRAGMA user_version = 6;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v7(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                ALTER TABLE quota_snapshots ADD COLUMN reset_credits_available INTEGER;
                PRAGMA user_version = 7;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }
}

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

impl AffinityRepository for SqliteStateStore {
    fn pin_account(
        &self,
        affinity_key: &AffinityKey,
        account_id: &AccountId,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO affinity_pins (affinity_key, account_id)
                 VALUES (?1, ?2)
                 ON CONFLICT(affinity_key) DO UPDATE SET
                   account_id = excluded.account_id",
                params![affinity_key.as_str(), account_id.as_str()],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn load_pin(&self, affinity_key: &AffinityKey) -> Result<Option<AccountId>, StateStoreError> {
        let account_id = self
            .connection
            .query_row(
                "SELECT account_id FROM affinity_pins WHERE affinity_key = ?1",
                params![affinity_key.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_error)?;

        account_id
            .map(AccountId::new)
            .transpose()
            .map_err(|_| StateStoreError::CorruptAccount {
                account_id: "<affinity-pin>".to_owned(),
                field: "account_id",
            })
    }

    fn write_previous_response_owner(
        &self,
        owner: &PreviousResponseAffinityOwnerRecord,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO previous_response_affinity_owners (
                   affinity_key_hash, route_band, account_id, credential_generation,
                   source_transport, created_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(affinity_key_hash, route_band, account_id) DO UPDATE SET
                   credential_generation = excluded.credential_generation,
                   source_transport = excluded.source_transport,
                   created_unix_seconds = excluded.created_unix_seconds",
                params![
                    owner.affinity_key_hash().as_str(),
                    owner.route_band().as_str(),
                    owner.account_id().as_str(),
                    u64_to_i64(owner.credential_generation())?,
                    owner.source_transport().as_str(),
                    u64_to_i64(owner.created_unix_seconds())?,
                ],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn load_previous_response_owner(
        &self,
        affinity_key_hash: &AffinityKeyHash,
        route_band: &str,
    ) -> Result<PreviousResponseAffinityOwnerLookup, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT affinity_key_hash, route_band, account_id, credential_generation,
                        source_transport, created_unix_seconds
                   FROM previous_response_affinity_owners
                  WHERE affinity_key_hash = ?1 AND route_band = ?2
                  ORDER BY account_id
                  LIMIT 2",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map(params![affinity_key_hash.as_str(), route_band], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut owners = Vec::new();
        for row in rows {
            let (
                hash_value,
                route_band_value,
                account_id_value,
                credential_generation,
                source_transport_value,
                created_unix_seconds,
            ) = row.map_err(sqlite_error)?;
            owners.push(parse_previous_response_owner_row(
                hash_value,
                route_band_value,
                account_id_value,
                credential_generation,
                source_transport_value,
                created_unix_seconds,
            )?);
        }

        match owners.len() {
            0 => Ok(PreviousResponseAffinityOwnerLookup::Missing),
            1 => Ok(PreviousResponseAffinityOwnerLookup::Found(owners.remove(0))),
            _ => Ok(PreviousResponseAffinityOwnerLookup::Ambiguous),
        }
    }

    fn purge_previous_response_owners(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute("DELETE FROM previous_response_affinity_owners", [])
            .map_err(sqlite_error)?;

        Ok(())
    }
}

fn sqlite_error(error: rusqlite::Error) -> StateStoreError {
    StateStoreError::Sqlite {
        message: error.to_string(),
    }
}

fn sqlx_error(error: sqlx::Error) -> StateStoreError {
    StateStoreError::Sqlite {
        message: error.to_string(),
    }
}

fn insert_selector_window_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
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

fn selector_windows_are_stale(
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

fn mark_selector_windows_stale(windows: &mut [PersistedSelectorQuotaWindow]) {
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

fn parse_refresh_error_class(value: &str) -> Result<QuotaRefreshErrorClass, StateStoreError> {
    QuotaRefreshErrorClass::parse(value).ok_or_else(|| StateStoreError::CorruptQuotaSnapshot {
        account_id: "<quota-refresh-status>".to_owned(),
        field: "last_error_class",
    })
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

fn parse_previous_response_owner_row(
    hash_value: String,
    route_band_value: String,
    account_id_value: String,
    credential_generation: i64,
    source_transport_value: String,
    created_unix_seconds: i64,
) -> Result<PreviousResponseAffinityOwnerRecord, StateStoreError> {
    let affinity_key_hash =
        AffinityKeyHash::new(hash_value).map_err(|_| StateStoreError::CorruptQuotaSnapshot {
            account_id: account_id_value.clone(),
            field: "affinity_key_hash",
        })?;
    let route_band = RouteBand::parse(&route_band_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaSnapshot {
            account_id: account_id_value.clone(),
            field: "route_band",
        }
    })?;
    let account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let credential_generation = i64_to_u64_account_generation(
        credential_generation,
        &account_id_value,
        "credential_generation",
    )?;
    let source_transport =
        AffinitySourceTransport::parse(&source_transport_value).ok_or_else(|| {
            StateStoreError::CorruptQuotaSnapshot {
                account_id: account_id_value.clone(),
                field: "source_transport",
            }
        })?;
    let created_unix_seconds = i64_to_u64(
        created_unix_seconds,
        &account_id_value,
        "created_unix_seconds",
    )?;

    Ok(PreviousResponseAffinityOwnerRecord::new(
        affinity_key_hash,
        account_id,
        credential_generation,
        route_band,
        source_transport,
        created_unix_seconds,
    ))
}

fn parse_account_row(
    account_id_value: String,
    label: String,
    status_value: String,
    active_credential_generation: Option<i64>,
) -> Result<AccountRecord, StateStoreError> {
    let parsed_account_id =
        AccountId::new(account_id_value.clone()).map_err(|_| StateStoreError::CorruptAccount {
            account_id: account_id_value.clone(),
            field: "account_id",
        })?;
    let status = AccountStatus::parse(&status_value).ok_or(StateStoreError::CorruptAccount {
        account_id: account_id_value,
        field: "status",
    })?;
    let active_credential_generation = active_credential_generation
        .map(|value| {
            i64_to_u64_account_generation(
                value,
                parsed_account_id.as_str(),
                "active_credential_generation",
            )
        })
        .transpose()?;

    let mut account = AccountRecord::new(parsed_account_id, label, status);
    if let Some(generation) = active_credential_generation {
        account = account.with_active_credential_generation(generation);
    }

    Ok(account)
}

fn selector_window_from_snapshot(
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

async fn invalidate_credential_mutation_quota_async(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    account_id: &AccountId,
) -> Result<(), StateStoreError> {
    for route_band in CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS {
        sqlx::query(
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
        )
        .bind(account_id.as_str())
        .bind(QuotaSnapshotSource::CredentialMutation.as_str())
        .bind(*route_band)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_error)?;
        sqlx::query(
            "DELETE FROM selector_quota_windows
              WHERE account_id = ?1 AND route_band = ?2",
        )
        .bind(account_id.as_str())
        .bind(*route_band)
        .execute(&mut **transaction)
        .await
        .map_err(sqlx_error)?;
        if selector_route_band(route_band) {
            sqlx::query(
                "INSERT INTO selector_quota_windows (
                   account_id, route_band, limit_window_seconds, status,
                   remaining_headroom, reset_unix_seconds, effective,
                   observed_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4, 0, NULL, 1, 0)",
            )
            .bind(account_id.as_str())
            .bind(*route_band)
            .bind(u64_to_i64(DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS)?)
            .bind(SelectorQuotaWindowStatus::Ineligible.as_str())
            .execute(&mut **transaction)
            .await
            .map_err(sqlx_error)?;
        }
    }

    Ok(())
}

fn invalidate_credential_mutation_quota_sync(
    transaction: &rusqlite::Transaction<'_>,
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

fn selector_route_band(route_band: &str) -> bool {
    SELECTOR_INVALIDATED_ROUTE_BANDS.contains(&route_band)
}

fn u64_to_i64(value: u64) -> Result<i64, StateStoreError> {
    i64::try_from(value).map_err(|_| StateStoreError::Sqlite {
        message: "u64 value does not fit sqlite integer".to_owned(),
    })
}

const fn u32_to_i64(value: u32) -> i64 {
    value as i64
}

fn i64_to_u64(value: i64, account_id: &str, field: &'static str) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptQuotaSnapshot {
        account_id: account_id.to_owned(),
        field,
    })
}

fn i64_to_u64_account_generation(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptAccount {
        account_id: account_id.to_owned(),
        field,
    })
}

fn i64_to_u32(value: i64, account_id: &str, field: &'static str) -> Result<u32, StateStoreError> {
    u32::try_from(value).map_err(|_| StateStoreError::CorruptQuotaSnapshot {
        account_id: account_id.to_owned(),
        field,
    })
}
