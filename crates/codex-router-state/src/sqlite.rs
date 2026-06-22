//! SQLite metadata store.

use std::fmt;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;
use rusqlite::Connection;
use rusqlite::OptionalExtension;
use rusqlite::params;
use thiserror::Error;

use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedSelectorQuotaWindow;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::SelectorQuotaInput;
use crate::quota_snapshot::SelectorQuotaWindowStatus;
use crate::repositories::AccountStateRepository;
use crate::repositories::AffinityRepository;
use crate::repositories::QuotaSnapshotRepository;
use crate::repositories::SelectorQuotaRepository;

const CURRENT_SCHEMA_VERSION: i64 = 3;
const CREDENTIAL_MUTATION_INVALIDATED_ROUTE_BANDS: &[&str] = &[
    "responses",
    "models",
    "memories_trace_summarize",
    "responses_compact",
    "code_review",
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
        }
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
        let stale_penalty = if snapshot.stale_penalty() {
            1_i64
        } else {
            0_i64
        };

        self.connection
            .execute(
                "INSERT INTO quota_snapshots (
                   account_id, source, observed_unix_seconds, route_band,
                   remaining_headroom, reset_unix_seconds, stale_penalty
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(account_id, route_band) DO UPDATE SET
                   source = excluded.source,
                   observed_unix_seconds = excluded.observed_unix_seconds,
                   remaining_headroom = excluded.remaining_headroom,
                   reset_unix_seconds = excluded.reset_unix_seconds,
                   stale_penalty = excluded.stale_penalty",
                params![
                    snapshot.account_id().as_str(),
                    snapshot.source().as_str(),
                    observed_unix_seconds,
                    snapshot.route_band(),
                    remaining_headroom,
                    reset_unix_seconds,
                    stale_penalty,
                ],
            )
            .map_err(sqlite_error)?;

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
                        remaining_headroom, reset_unix_seconds, stale_penalty
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
                        row.get::<_, i64>(6)?,
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

    /// Loads selector input rows for one route band.
    pub fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        let accounts = self.list_accounts()?;
        let mut inputs = Vec::new();
        for account in accounts {
            let windows = self.load_selector_windows(account.account_id(), route_band)?;
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
                self.apply_v3()
            }
            2 => self.apply_v3(),
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

                PRAGMA user_version = 3;
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
        self.connection
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

                PRAGMA user_version = 3;
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

    fn selector_inputs_for_route_band(
        &self,
        route_band: &str,
    ) -> Result<Vec<SelectorQuotaInput>, StateStoreError> {
        self.selector_inputs_for_route_band(route_band)
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
}

fn sqlite_error(error: rusqlite::Error) -> StateStoreError {
    StateStoreError::Sqlite {
        message: error.to_string(),
    }
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
