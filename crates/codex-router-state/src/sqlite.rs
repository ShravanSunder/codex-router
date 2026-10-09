//! SQLite metadata store.

mod account_store;
mod active_leases;
mod affinity_store;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod legacy_schema_v1to5;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod legacy_schema_v6to10;
mod policy_mutation;
mod quota_history;
mod quota_refresh_status;
mod quota_snapshots;
mod repository_contracts;
mod selector_windows;
mod session_history;
mod session_records;
mod session_rollups;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod sync_accounts;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod sync_affinity;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod sync_migrations;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod sync_selectors;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
mod sync_snapshots;
pub(crate) use account_store::invalidate_credential_mutation_quota_async;
use account_store::parse_account_row;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use legacy_schema_v6to10::{
    ASYNC_ACTIVE_CLIENT_SCHEMA_STATEMENTS, ASYNC_ACTIVE_SESSION_HISTORY_INDEX_STATEMENTS,
    ASYNC_ACTIVE_SESSION_HISTORY_TABLE_STATEMENTS, ASYNC_CREDIT_USAGE_SCHEMA_STATEMENTS,
    ASYNC_QUOTA_HISTORY_SCHEMA_STATEMENTS, ASYNC_ROUTE_BAND_ACCOUNT_STATE_SCHEMA_STATEMENTS,
};
pub(crate) use quota_history::insert_quota_history_observation_in_async_transaction;
pub(crate) use quota_snapshots::upsert_quota_snapshot_in_async_transaction;
pub use repository_contracts::{
    AsyncAffinityRepository, AsyncQuotaExhaustionRepository, AsyncQuotaHistoryRepository,
    AsyncSelectorQuotaRepository, AsyncSessionAccountAffinityRepository,
};
use selector_windows::insert_selector_window_in_async_transaction;
use selector_windows::{DEFAULT_SELECTOR_LIMIT_WINDOW_SECONDS, SELECTOR_INVALIDATED_ROUTE_BANDS};
pub use session_records::{
    ActiveClientCount, ActiveSessionEvent, ActiveSessionEventKind, ActiveSessionRollup,
};
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use sync_selectors::invalidate_credential_mutation_quota_sync;

use std::collections::BTreeMap;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use codex_router_core::affinity::AffinityKeyHash;
use codex_router_core::ids::AccountId;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use codex_router_core::ids::AffinityKey;
use codex_router_core::ids::ReservationId;
use codex_router_core::provider::Provider;
use codex_router_core::routes::RouteBand;
use futures_util::future::BoxFuture;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use rusqlite::Connection;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use rusqlite::OptionalExtension;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use rusqlite::params;
use sqlx::Row;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::sqlite::SqliteJournalMode;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::sqlite::SqliteRow;
use thiserror::Error;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::account_routing_policy::AccountRoutingPolicy;
use crate::account_routing_policy::WeeklyQuotaFloorBasisPoints;
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
use crate::quota_snapshot::SelectorCredentialMaintenance;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use crate::repositories::AccountStateRepository;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use crate::repositories::AffinityRepository;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use crate::repositories::QuotaSnapshotRepository;
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
use crate::repositories::SelectorQuotaRepository;
use crate::session_account_affinity::PinObservation;
use crate::session_account_affinity::SessionAccountAffinity;

const CURRENT_SCHEMA_VERSION: i64 = 13;

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
    /// Weekly-floor mutation requires a router-migrated v12 database.
    #[error("weekly quota floor requires a compatible upgraded router database")]
    WeeklyQuotaFloorSchemaUpgradeRequired,
    /// Weekly-floor mutation could not acquire the SQLite writer lock in time.
    #[error("database is busy; retry the command")]
    WeeklyQuotaFloorDatabaseBusy,
    /// Weekly-floor mutation named an account that is not configured.
    #[error("weekly quota floor account was not found")]
    WeeklyQuotaFloorAccountNotFound,
    /// Weekly-floor mutation matched more than one configured display label.
    #[error("weekly quota floor account label is ambiguous")]
    WeeklyQuotaFloorAccountLabelAmbiguous,
    /// Account-status mutation requires a router-migrated current database.
    #[error("account status requires a compatible upgraded router database")]
    AccountStatusSchemaUpgradeRequired,
    /// Account-status mutation could not acquire the SQLite writer lock in time.
    #[error("account status update is busy; retry the command")]
    AccountStatusDatabaseBusy,
    /// Account-status mutation named an account that is not configured.
    #[error("account status target was not found")]
    AccountStatusAccountNotFound,
    /// Credit policy mutation requires an existing native database with the current schema.
    #[error("credit preference requires a compatible upgraded router database")]
    CreditUsagePolicySchemaUpgradeRequired,
    /// Credit policy mutation could not acquire the SQLite writer lock.
    #[error("database is busy; retry save")]
    CreditUsagePolicyDatabaseBusy,
    /// Credit policy mutation named an account that is not configured.
    #[error("credit preference account is unavailable")]
    CreditUsagePolicyAccountUnavailable,
    /// Credit policy mutation target changed after the account pane was opened.
    #[error("account credentials changed; reopen account options")]
    CreditUsagePolicyTargetChanged,
    /// Account-status mutation matched more than one configured display label.
    #[error("account status target label is ambiguous")]
    AccountStatusAccountLabelAmbiguous,
    /// An account's persisted provider cannot be changed by an upsert.
    #[error("account provider is immutable")]
    AccountProviderImmutable,
    /// Persisted weekly-floor policy is outside the supported integer-percent range.
    #[error("stored weekly quota floor policy is invalid")]
    CorruptAccountRoutingPolicy,
    /// Read-only open found a current-version database missing required schema.
    #[error(
        "sqlite state store requires writable schema upgrade: missing {object_kind} {object_name}"
    )]
    MissingReadOnlySchemaObject {
        /// Missing SQLite object kind.
        object_kind: &'static str,
        /// Missing SQLite object name.
        object_name: &'static str,
    },
    /// Account metadata is corrupt; affected account fails closed.
    #[error("corrupt account metadata for {account_id}: {field}")]
    CorruptAccount {
        /// Affected account id.
        account_id: String,
        /// Corrupt field name.
        field: &'static str,
    },
    /// Session-to-account affinity metadata is corrupt.
    #[error("corrupt session account affinity for {session_id}: {field}")]
    CorruptSessionAccountAffinity {
        /// Affected provider session id.
        session_id: String,
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
    /// The per-account credit refresh attempt sequence reached SQLite's integer limit.
    #[error("credit refresh attempt sequence overflow")]
    CreditRefreshAttemptSequenceOverflow,
    /// A paired Responses refresh commit did not satisfy the state-owned contract.
    #[error("invalid credit refresh input: {field}")]
    InvalidCreditRefreshInput {
        /// Invalid input field.
        field: &'static str,
    },
    /// A Claude window observation or rejection contains an invalid input value.
    #[error("invalid Claude account window state: {field}")]
    InvalidAccountWindowState {
        /// Invalid field.
        field: &'static str,
    },
    /// Persisted Claude window state is corrupt; selection must fail closed.
    #[error("corrupt Claude account window state for {account_id}: {field}")]
    CorruptAccountWindowState {
        /// Affected account id.
        account_id: String,
        /// Corrupt field.
        field: &'static str,
    },
    /// Claude window state was requested for an account that does not exist.
    #[error("Claude window state account was not found")]
    AccountWindowStateAccountNotFound,
    /// Claude quota windows may only be stored for Claude accounts.
    #[error("Claude window state requires a Claude account")]
    AccountWindowStateRequiresClaudeAccount,
}

/// SQLite-backed metadata repository.
#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub struct SqliteStateStore {
    database_path: PathBuf,
    connection: Connection,
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
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
    pub(crate) pool: sqlx::SqlitePool,
    pub(crate) read_only: bool,
}

/// Committed result of a weekly-floor mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WeeklyQuotaFloorMutationResult {
    /// The floor is enabled with this committed policy.
    Enabled(AccountRoutingPolicy),
    /// The policy row was deleted and the floor is disabled.
    Disabled,
}

/// Narrow writable connection used only by the account weekly-floor setter.
#[derive(Debug)]
pub struct AsyncWeeklyQuotaFloorMutationStore {
    pool: sqlx::SqlitePool,
}

impl AsyncSqliteStateStore {
    /// Inspects native or recognized bootstrap state for Fresh startup, without applying it.
    pub async fn prepare_startup_schema(
        database_path: &Path,
    ) -> Result<
        crate::schema_preparation::AccountStartupSchemaPreparation,
        crate::schema_preparation::StateSchemaPreparationError,
    > {
        crate::schema_preparation::prepare_startup_schema(database_path).await
    }

    /// Inspects an existing account database without applying migrations or opening a store.
    pub async fn prepare_schema(
        database_path: &Path,
    ) -> Result<
        crate::schema_preparation::AccountSchemaPreparation,
        crate::schema_preparation::StateSchemaPreparationError,
    > {
        crate::schema_preparation::prepare_schema(database_path).await
    }

    /// Opens a SQLite state database through SQLx and applies supported migrations.
    pub async fn open(database_path: &Path) -> Result<Self, StateStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(sqlx_error)?;
        let store = Self {
            database_path: database_path.to_path_buf(),
            pool,
            read_only: false,
        };
        if let Err(error) = crate::account_migrations::migrate(&store.pool).await {
            store.pool.close().await;
            return Err(error);
        }

        Ok(store)
    }

    /// Opens an existing SQLite state database for read-only status/reporting paths.
    ///
    /// This intentionally does not apply migrations or create missing tables. The serve process is
    /// the database writer; read-only CLI status commands must not request write locks.
    pub async fn open_read_only(database_path: &Path) -> Result<Self, StateStoreError> {
        let options = SqliteConnectOptions::new()
            .filename(database_path)
            .read_only(true)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(0))
            .pragma("query_only", "ON");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .map_err(sqlx_error)?;
        let store = Self {
            database_path: database_path.to_path_buf(),
            pool,
            read_only: true,
        };
        let authority = {
            let mut connection = store.pool.acquire().await.map_err(sqlx_error)?;
            crate::account_migrations::migration_authority(&mut connection).await?
        };
        let validation = match authority {
            crate::account_migrations::MigrationAuthority::Legacy => {
                let version = store.schema_version().await?;
                if version != CURRENT_SCHEMA_VERSION {
                    Err(StateStoreError::UnsupportedSchemaVersion { version })
                } else {
                    let mut connection = store.pool.acquire().await.map_err(sqlx_error)?;
                    crate::account_schema::validate_required_read_only_objects(&mut connection)
                        .await
                }
            }
            crate::account_migrations::MigrationAuthority::NativeCurrent => {
                let mut connection = store.pool.acquire().await.map_err(sqlx_error)?;
                crate::account_schema::validate_required_read_only_objects(&mut connection).await?;
                crate::account_migrations::validate_native_read_only_schema(&mut connection).await
            }
            crate::account_migrations::MigrationAuthority::NativeUpgradeRequired => {
                let mut connection = store.pool.acquire().await.map_err(sqlx_error)?;
                crate::account_migrations::validate_native_read_only_schema(&mut connection).await
            }
        };
        if let Err(error) = validation {
            store.pool.close().await;
            Err(error)
        } else {
            Ok(store)
        }
    }

    /// Returns the database path used by this async store.
    #[must_use]
    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    /// Closes the underlying SQLx pool and waits for checked-in connections to close.
    pub async fn close(&self) -> Result<(), StateStoreError> {
        if !self.read_only {
            sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
                .execute(&self.pool)
                .await
                .map_err(sqlx_error)?;
        }
        self.pool.close().await;
        Ok(())
    }

    /// Returns the active schema version.
    pub async fn schema_version(&self) -> Result<i64, StateStoreError> {
        sqlx::query("PRAGMA user_version")
            .fetch_one(&self.pool)
            .await
            .map(|row| row.get::<i64, _>(0))
            .map_err(sqlx_error)
    }

    /// Acquires and holds one connection from this store's pool for isolation tests.
    #[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
    pub async fn acquire_connection_for_test(
        &self,
    ) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>, StateStoreError> {
        self.pool.acquire().await.map_err(sqlx_error)
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
fn sqlite_error(error: rusqlite::Error) -> StateStoreError {
    StateStoreError::Sqlite {
        message: error.to_string(),
    }
}

pub(crate) fn sqlx_error(error: sqlx::Error) -> StateStoreError {
    StateStoreError::Sqlite {
        message: error.to_string(),
    }
}

pub(crate) fn is_busy_or_locked_sqlx_error(error: &sqlx::Error) -> bool {
    let sqlx::Error::Database(database_error) = error else {
        return false;
    };
    matches!(database_error.code().as_deref(), Some("5" | "6"))
        || database_error.message().contains("database is locked")
        || database_error
            .message()
            .contains("database table is locked")
}

fn selector_route_band(route_band: &str) -> bool {
    SELECTOR_INVALIDATED_ROUTE_BANDS.contains(&route_band)
}

pub(crate) fn u64_to_i64(value: u64) -> Result<i64, StateStoreError> {
    i64::try_from(value).map_err(|_| StateStoreError::Sqlite {
        message: "u64 value does not fit sqlite integer".to_owned(),
    })
}

pub(crate) fn i64_to_u64_window_state(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptAccountWindowState {
        account_id: account_id.to_owned(),
        field,
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
