//! SQLite metadata store.

use std::fmt;
use std::fs;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::ids::AccountId;
use codex_router_core::ids::AffinityKey;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use rusqlite::OptionalExtension;
use rusqlite::params;
use thiserror::Error;

use crate::account::AccountCredentialMetadata;
use crate::account::AccountRecord;
use crate::account::AccountStatus;
use crate::quota_snapshot::PersistedQuotaSnapshot;
use crate::quota_snapshot::PersistedQuotaStatusRow;
use crate::quota_snapshot::QuotaSnapshotSource;
use crate::quota_snapshot::QuotaStatusState;
use crate::repositories::AccountCredentialRepository;
use crate::repositories::AccountStateRepository;
use crate::repositories::AffinityRepository;
use crate::repositories::QuotaSnapshotRepository;
use crate::repositories::QuotaStatusRepository;

const CURRENT_SCHEMA_VERSION: i64 = 2;

/// SQLite state store failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum StateStoreError {
    /// SQLite failed.
    #[error("sqlite state store failed: {message}")]
    Sqlite {
        /// Redacted SQLite error message.
        message: String,
    },
    /// State database path must not be inside Codex-owned home state.
    #[error("sqlite state path must not be inside .codex or .prodex: {}", path.display())]
    CodexHomePath {
        /// Rejected path.
        path: PathBuf,
    },
    /// State database path must not traverse symlinks.
    #[error("sqlite state path must not traverse symlink: {}", path.display())]
    SymlinkPath {
        /// Rejected path component.
        path: PathBuf,
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
    /// Account credential metadata is corrupt; affected account fails closed.
    #[error("corrupt account credential metadata for {account_id}: {field}")]
    CorruptAccountCredential {
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
    /// Quota status metadata is corrupt; affected row fails closed.
    #[error("corrupt quota status metadata for {account_id}: {field}")]
    CorruptQuotaStatus {
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
        validate_state_database_path(database_path)?;
        let connection = Connection::open(database_path).map_err(sqlite_error)?;
        let store = Self {
            database_path: database_path.to_path_buf(),
            connection,
        };
        store.migrate()?;

        Ok(store)
    }

    /// Opens an existing SQLite state database without creating or migrating it.
    pub fn open_existing_read_only(database_path: &Path) -> Result<Self, StateStoreError> {
        validate_state_database_path(database_path)?;
        if !database_path.exists() {
            return Err(StateStoreError::Sqlite {
                message: format!(
                    "sqlite state database does not exist: {}",
                    database_path.display()
                ),
            });
        }
        let connection =
            Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(sqlite_error)?;
        let version = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sqlite_error)?;
        if version != CURRENT_SCHEMA_VERSION {
            return Err(StateStoreError::UnsupportedSchemaVersion { version });
        }

        Ok(Self {
            database_path: database_path.to_path_buf(),
            connection,
        })
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
                "INSERT INTO accounts (account_id, label, status)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id) DO UPDATE SET
                   label = excluded.label,
                   status = excluded.status",
                params![
                    account.account_id().as_str(),
                    account.label(),
                    account.status().as_str()
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
                "SELECT account_id, label, status FROM accounts WHERE account_id = ?1",
                params![account_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some((account_id_value, label, status_value)) = row else {
            return Ok(None);
        };

        parse_account_row(account_id_value, label, status_value).map(Some)
    }

    /// Lists account metadata in deterministic selector order.
    pub fn list_accounts(&self) -> Result<Vec<AccountRecord>, StateStoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT account_id, label, status FROM accounts ORDER BY account_id")
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut accounts = Vec::new();
        for row in rows {
            let (account_id_value, label, status_value) = row.map_err(sqlite_error)?;
            accounts.push(parse_account_row(account_id_value, label, status_value)?);
        }

        Ok(accounts)
    }

    /// Inserts or updates non-secret account credential metadata.
    pub fn upsert_credential_metadata(
        &self,
        metadata: &AccountCredentialMetadata,
    ) -> Result<(), StateStoreError> {
        let has_refresh_token = bool_to_i64(metadata.has_refresh_token());
        let expires_at_unix_seconds = metadata
            .expires_at_unix_seconds()
            .map(u64_to_i64)
            .transpose()?;
        let updated_unix_seconds = u64_to_i64(metadata.updated_unix_seconds())?;

        self.connection
            .execute(
                "INSERT INTO account_credentials (
                   account_id, has_refresh_token, expires_at_unix_seconds, updated_unix_seconds
                 )
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET
                   has_refresh_token = excluded.has_refresh_token,
                   expires_at_unix_seconds = excluded.expires_at_unix_seconds,
                   updated_unix_seconds = excluded.updated_unix_seconds",
                params![
                    metadata.account_id().as_str(),
                    has_refresh_token,
                    expires_at_unix_seconds,
                    updated_unix_seconds,
                ],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    /// Loads non-secret account credential metadata.
    pub fn load_credential_metadata(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountCredentialMetadata>, StateStoreError> {
        let row = self
            .connection
            .query_row(
                "SELECT account_id, has_refresh_token, expires_at_unix_seconds, updated_unix_seconds
                   FROM account_credentials
                  WHERE account_id = ?1",
                params![account_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(sqlite_error)?;

        let Some(row) = row else {
            return Ok(None);
        };

        parse_account_credential_metadata(row).map(Some)
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

    /// Inserts or updates a persisted quota status row.
    pub fn upsert_quota_status_row(
        &self,
        row: &PersistedQuotaStatusRow,
    ) -> Result<(), StateStoreError> {
        upsert_quota_status_row(&self.connection, row)
    }

    /// Lists persisted quota status rows in deterministic display order.
    pub fn list_quota_status_rows(&self) -> Result<Vec<PersistedQuotaStatusRow>, StateStoreError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT account_id, source, observed_unix_seconds, route_band,
                        family, window_label, status, used_percent, remaining_headroom,
                        reset_unix_seconds, limit_window_seconds, effective,
                        failure_message, failure_unix_seconds
                   FROM quota_status_rows
                  ORDER BY account_id, route_band, family, effective DESC, window_label",
            )
            .map_err(sqlite_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                    row.get::<_, Option<i64>>(10)?,
                    row.get::<_, i64>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<i64>>(13)?,
                ))
            })
            .map_err(sqlite_error)?;

        let mut status_rows = Vec::new();
        for row in rows {
            let row = row.map_err(sqlite_error)?;
            status_rows.push(parse_quota_status_row(row)?);
        }

        Ok(status_rows)
    }

    /// Replaces one account/route-band selector snapshot and status rows atomically.
    pub fn replace_route_quota_state(
        &self,
        snapshot: &PersistedQuotaSnapshot,
        status_rows: &[PersistedQuotaStatusRow],
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE TRANSACTION")
            .map_err(sqlite_error)?;

        let result = (|| {
            self.upsert_quota_snapshot(snapshot)?;
            self.connection
                .execute(
                    "DELETE FROM quota_status_rows
                      WHERE account_id = ?1 AND route_band = ?2",
                    params![snapshot.account_id().as_str(), snapshot.route_band()],
                )
                .map_err(sqlite_error)?;
            for row in status_rows {
                validate_status_row_matches_snapshot(row, snapshot)?;
                upsert_quota_status_row(&self.connection, row)?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.connection
                    .execute_batch("COMMIT")
                    .map_err(sqlite_error)?;
                Ok(())
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
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

    /// Inserts raw quota status metadata for corruption fixtures.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn insert_raw_quota_status_for_test(
        &self,
        account_id: &str,
        source: &str,
        observed_unix_seconds: i64,
        route_band: &str,
        family: &str,
        window_label: &str,
        status: &str,
        used_percent: Option<i64>,
        remaining_headroom: i64,
        effective: i64,
    ) -> Result<(), StateStoreError> {
        self.connection
            .execute(
                "INSERT INTO quota_status_rows (
                   account_id, source, observed_unix_seconds, route_band,
                   family, window_label, status, used_percent, remaining_headroom,
                   effective
                 )
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    account_id,
                    source,
                    observed_unix_seconds,
                    route_band,
                    family,
                    window_label,
                    status,
                    used_percent,
                    remaining_headroom,
                    effective,
                ],
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn migrate(&self) -> Result<(), StateStoreError> {
        let version = self.schema_version();
        match version {
            0 => {
                self.apply_v1()?;
                self.apply_v2()
            }
            1 => self.apply_v2(),
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
                    status TEXT NOT NULL
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

                PRAGMA user_version = 1;
                ",
            )
            .map_err(sqlite_error)?;

        Ok(())
    }

    fn apply_v2(&self) -> Result<(), StateStoreError> {
        self.connection
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS quota_status_rows (
                    account_id TEXT NOT NULL,
                    source TEXT NOT NULL,
                    observed_unix_seconds INTEGER NOT NULL,
                    route_band TEXT NOT NULL,
                    family TEXT NOT NULL,
                    window_label TEXT NOT NULL,
                    status TEXT NOT NULL,
                    used_percent INTEGER,
                    remaining_headroom INTEGER NOT NULL,
                    reset_unix_seconds INTEGER,
                    limit_window_seconds INTEGER,
                    effective INTEGER NOT NULL,
                    failure_message TEXT,
                    failure_unix_seconds INTEGER,
                    PRIMARY KEY (account_id, route_band, family, window_label)
                );

                CREATE TABLE IF NOT EXISTS account_credentials (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    has_refresh_token INTEGER NOT NULL,
                    expires_at_unix_seconds INTEGER,
                    updated_unix_seconds INTEGER NOT NULL
                );

                PRAGMA user_version = 2;
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

impl AccountCredentialRepository for SqliteStateStore {
    fn upsert_credential_metadata(
        &self,
        metadata: &AccountCredentialMetadata,
    ) -> Result<(), StateStoreError> {
        self.upsert_credential_metadata(metadata)
    }

    fn load_credential_metadata(
        &self,
        account_id: &AccountId,
    ) -> Result<Option<AccountCredentialMetadata>, StateStoreError> {
        self.load_credential_metadata(account_id)
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

impl QuotaStatusRepository for SqliteStateStore {
    fn upsert_status_row(&self, row: &PersistedQuotaStatusRow) -> Result<(), StateStoreError> {
        self.upsert_quota_status_row(row)
    }

    fn list_status_rows(&self) -> Result<Vec<PersistedQuotaStatusRow>, StateStoreError> {
        self.list_quota_status_rows()
    }

    fn replace_route_quota_state(
        &self,
        snapshot: &PersistedQuotaSnapshot,
        status_rows: &[PersistedQuotaStatusRow],
    ) -> Result<(), StateStoreError> {
        self.replace_route_quota_state(snapshot, status_rows)
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

fn upsert_quota_status_row(
    connection: &Connection,
    row: &PersistedQuotaStatusRow,
) -> Result<(), StateStoreError> {
    let observed_unix_seconds = u64_to_i64(row.observed_unix_seconds())?;
    let used_percent = row.used_percent().map(u32_to_i64);
    let remaining_headroom = u32_to_i64(row.remaining_headroom());
    let reset_unix_seconds = row.reset_unix_seconds().map(u64_to_i64).transpose()?;
    let limit_window_seconds = row.limit_window_seconds().map(u64_to_i64).transpose()?;
    let effective = bool_to_i64(row.effective());
    let failure_unix_seconds = row.failure_unix_seconds().map(u64_to_i64).transpose()?;

    connection
        .execute(
            "INSERT INTO quota_status_rows (
               account_id, source, observed_unix_seconds, route_band,
               family, window_label, status, used_percent, remaining_headroom,
               reset_unix_seconds, limit_window_seconds, effective,
               failure_message, failure_unix_seconds
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(account_id, route_band, family, window_label) DO UPDATE SET
               source = excluded.source,
               observed_unix_seconds = excluded.observed_unix_seconds,
               status = excluded.status,
               used_percent = excluded.used_percent,
               remaining_headroom = excluded.remaining_headroom,
               reset_unix_seconds = excluded.reset_unix_seconds,
               limit_window_seconds = excluded.limit_window_seconds,
               effective = excluded.effective,
               failure_message = excluded.failure_message,
               failure_unix_seconds = excluded.failure_unix_seconds",
            params![
                row.account_id().as_str(),
                row.source().as_str(),
                observed_unix_seconds,
                row.route_band(),
                row.family(),
                row.window_label(),
                row.status().as_str(),
                used_percent,
                remaining_headroom,
                reset_unix_seconds,
                limit_window_seconds,
                effective,
                row.failure_message(),
                failure_unix_seconds,
            ],
        )
        .map_err(sqlite_error)?;

    Ok(())
}

type RawQuotaStatusRow = (
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    Option<i64>,
    i64,
    Option<i64>,
    Option<i64>,
    i64,
    Option<String>,
    Option<i64>,
);

fn parse_quota_status_row(
    row: RawQuotaStatusRow,
) -> Result<PersistedQuotaStatusRow, StateStoreError> {
    let (
        account_id_value,
        source_value,
        observed_unix_seconds,
        route_band,
        family,
        window_label,
        status_value,
        used_percent,
        remaining_headroom,
        reset_unix_seconds,
        limit_window_seconds,
        effective,
        failure_message,
        failure_unix_seconds,
    ) = row;

    let parsed_account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
        StateStoreError::CorruptQuotaStatus {
            account_id: account_id_value.clone(),
            field: "account_id",
        }
    })?;
    let source = QuotaSnapshotSource::parse(&source_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaStatus {
            account_id: account_id_value.clone(),
            field: "source",
        }
    })?;
    let observed = quota_status_i64_to_u64(
        observed_unix_seconds,
        &account_id_value,
        "observed_unix_seconds",
    )?;
    let status = QuotaStatusState::parse(&status_value).ok_or_else(|| {
        StateStoreError::CorruptQuotaStatus {
            account_id: account_id_value.clone(),
            field: "status",
        }
    })?;
    let used = used_percent
        .map(|value| quota_status_percent_to_u32(value, &account_id_value, "used_percent"))
        .transpose()?;
    let remaining =
        quota_status_percent_to_u32(remaining_headroom, &account_id_value, "remaining_headroom")?;
    let reset = reset_unix_seconds
        .map(|value| quota_status_i64_to_u64(value, &account_id_value, "reset_unix_seconds"))
        .transpose()?;
    let limit = limit_window_seconds
        .map(|value| quota_status_i64_to_u64(value, &account_id_value, "limit_window_seconds"))
        .transpose()?;
    let is_effective = sqlite_bool_to_bool(effective, &account_id_value, "effective")?;
    let failure_time = failure_unix_seconds
        .map(|value| quota_status_i64_to_u64(value, &account_id_value, "failure_unix_seconds"))
        .transpose()?;

    let mut parsed =
        PersistedQuotaStatusRow::new(parsed_account_id, source, route_band, family, window_label)
            .with_observed_unix_seconds(observed)
            .with_status(status)
            .with_remaining_headroom(remaining)
            .with_effective(is_effective);
    if let Some(used) = used {
        parsed = parsed.with_used_percent(used);
    }
    if let Some(reset) = reset {
        parsed = parsed.with_reset_unix_seconds(reset);
    }
    if let Some(limit) = limit {
        parsed = parsed.with_limit_window_seconds(limit);
    }
    if let (Some(message), Some(failure_time)) = (failure_message, failure_time) {
        parsed = parsed.with_failure(message, failure_time);
    }

    Ok(parsed)
}

fn validate_status_row_matches_snapshot(
    row: &PersistedQuotaStatusRow,
    snapshot: &PersistedQuotaSnapshot,
) -> Result<(), StateStoreError> {
    if row.account_id() != snapshot.account_id() {
        return Err(StateStoreError::CorruptQuotaStatus {
            account_id: row.account_id().as_str().to_owned(),
            field: "account_id_mismatch",
        });
    }
    if row.route_band() != snapshot.route_band() {
        return Err(StateStoreError::CorruptQuotaStatus {
            account_id: row.account_id().as_str().to_owned(),
            field: "route_band_mismatch",
        });
    }

    Ok(())
}

fn quota_status_i64_to_u64(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptQuotaStatus {
        account_id: account_id.to_owned(),
        field,
    })
}

fn quota_status_percent_to_u32(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<u32, StateStoreError> {
    let parsed = u32::try_from(value).map_err(|_| StateStoreError::CorruptQuotaStatus {
        account_id: account_id.to_owned(),
        field,
    })?;
    if parsed > 100 {
        return Err(StateStoreError::CorruptQuotaStatus {
            account_id: account_id.to_owned(),
            field,
        });
    }

    Ok(parsed)
}

const fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn sqlite_bool_to_bool(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<bool, StateStoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(StateStoreError::CorruptQuotaStatus {
            account_id: account_id.to_owned(),
            field,
        }),
    }
}

fn validate_state_database_path(database_path: &Path) -> Result<(), StateStoreError> {
    reject_codex_home_path(database_path)?;
    reject_symlink_path(database_path)?;
    validate_existing_parent(database_path)
}

fn reject_codex_home_path(path: &Path) -> Result<(), StateStoreError> {
    if path.components().any(is_codex_or_prodex_component) {
        return Err(StateStoreError::CodexHomePath {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn is_codex_or_prodex_component(component: Component<'_>) -> bool {
    matches!(component, Component::Normal(value) if value == ".codex" || value == ".prodex")
}

fn reject_symlink_path(path: &Path) -> Result<(), StateStoreError> {
    if path_is_symlink(path)? {
        return Err(StateStoreError::SymlinkPath {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn validate_existing_parent(path: &Path) -> Result<(), StateStoreError> {
    let mut current_path = path.parent();
    while let Some(parent) = current_path {
        reject_symlink_path(parent)?;
        if parent.exists() {
            return Ok(());
        }
        current_path = parent.parent();
    }

    Ok(())
}

fn path_is_symlink(path: &Path) -> Result<bool, StateStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(StateStoreError::Sqlite {
            message: format!("failed to inspect {}: {error}", path.display()),
        }),
    }
}

fn parse_account_row(
    account_id_value: String,
    label: String,
    status_value: String,
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

    Ok(AccountRecord::new(parsed_account_id, label, status))
}

fn parse_account_credential_metadata(
    row: (String, i64, Option<i64>, i64),
) -> Result<AccountCredentialMetadata, StateStoreError> {
    let (account_id_value, has_refresh_token, expires_at_unix_seconds, updated_unix_seconds) = row;
    let parsed_account_id = AccountId::new(account_id_value.clone()).map_err(|_| {
        StateStoreError::CorruptAccountCredential {
            account_id: account_id_value.clone(),
            field: "account_id",
        }
    })?;
    let has_refresh_token = match has_refresh_token {
        0 => false,
        1 => true,
        _ => {
            return Err(StateStoreError::CorruptAccountCredential {
                account_id: account_id_value,
                field: "has_refresh_token",
            });
        }
    };
    let expires_at_unix_seconds = expires_at_unix_seconds
        .map(|value| {
            account_credential_i64_to_u64(value, &account_id_value, "expires_at_unix_seconds")
        })
        .transpose()?;
    let updated_unix_seconds = account_credential_i64_to_u64(
        updated_unix_seconds,
        &account_id_value,
        "updated_unix_seconds",
    )?;

    Ok(AccountCredentialMetadata::new(
        parsed_account_id,
        has_refresh_token,
        expires_at_unix_seconds,
        updated_unix_seconds,
    ))
}

fn account_credential_i64_to_u64(
    value: i64,
    account_id: &str,
    field: &'static str,
) -> Result<u64, StateStoreError> {
    u64::try_from(value).map_err(|_| StateStoreError::CorruptAccountCredential {
        account_id: account_id.to_owned(),
        field,
    })
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

fn i64_to_u32(value: i64, account_id: &str, field: &'static str) -> Result<u32, StateStoreError> {
    u32::try_from(value).map_err(|_| StateStoreError::CorruptQuotaSnapshot {
        account_id: account_id.to_owned(),
        field,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::SqliteStateStore;
    use super::StateStoreError;

    #[test]
    fn sqlite_state_store_rejects_prodex_paths() {
        let root = std::env::temp_dir().join(format!(
            "codex-router-state-prodex-test-{}",
            std::process::id()
        ));
        let _cleanup_before = fs::remove_dir_all(&root);
        let database_path = root.join(".prodex").join("state.sqlite");

        let result = SqliteStateStore::open(&database_path);

        match result {
            Err(StateStoreError::CodexHomePath { path }) => {
                assert_eq!(path, database_path);
            }
            Ok(_) => panic!("state store should reject .prodex paths"),
            Err(error) => panic!("unexpected state store error: {error}"),
        }
        let _cleanup_after = fs::remove_dir_all(&root);
    }
}
