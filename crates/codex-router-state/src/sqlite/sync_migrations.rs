//! SQLite sync migrations responsibilities.
use super::policy_mutation::redacted_weekly_floor_sqlite_error;
use super::*;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const PREVIOUS_SCHEMA_VERSION: i64 = 12;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const SCHEMA_VERSION_V11: i64 = 11;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
pub(super) const SCHEMA_VERSION_V10: i64 = 10;

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
const V11_ACCOUNT_ROUTING_POLICY_TABLE_SQL: &str = "CREATE TABLE account_routing_policies (
    account_id TEXT PRIMARY KEY NOT NULL,
    weekly_quota_floor_basis_points INTEGER NOT NULL
        CHECK (
            weekly_quota_floor_basis_points BETWEEN 100 AND 1000
            AND weekly_quota_floor_basis_points % 100 = 0
        )
)";

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
const V12_ACCOUNT_ROUTING_POLICY_REPLACEMENT_TABLE_SQL: &str =
    "CREATE TABLE account_routing_policies_v12 (
        account_id TEXT PRIMARY KEY NOT NULL,
        weekly_quota_floor_basis_points INTEGER NOT NULL
            CHECK (
                weekly_quota_floor_basis_points BETWEEN 100 AND 1500
                AND weekly_quota_floor_basis_points % 100 = 0
            )
    )";

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
const V13_SESSION_ACCOUNT_AFFINITY_TABLE_SQL: &str = "CREATE TABLE session_account_affinities (
        session_id TEXT PRIMARY KEY NOT NULL,
        account_id TEXT NOT NULL,
        last_seen_unix_seconds INTEGER NOT NULL
    )";

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
fn rebuild_account_routing_policy_table_v12_sync(
    transaction: &Connection,
) -> Result<(), StateStoreError> {
    transaction
        .execute(V12_ACCOUNT_ROUTING_POLICY_REPLACEMENT_TABLE_SQL, [])
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "INSERT INTO account_routing_policies_v12 (
                account_id, weekly_quota_floor_basis_points
             )
             SELECT account_id, weekly_quota_floor_basis_points
               FROM account_routing_policies",
            [],
        )
        .map_err(sqlite_error)?;
    transaction
        .execute("DROP TABLE account_routing_policies", [])
        .map_err(sqlite_error)?;
    transaction
        .execute(
            "ALTER TABLE account_routing_policies_v12 RENAME TO account_routing_policies",
            [],
        )
        .map_err(sqlite_error)?;
    transaction
        .pragma_update(None, "user_version", PREVIOUS_SCHEMA_VERSION)
        .map_err(sqlite_error)?;

    Ok(())
}

impl SqliteStateStore {
    pub(super) fn migrate(&self) -> Result<(), StateStoreError> {
        let version = self.schema_version();
        match version {
            0 => self.apply_v1(),
            1 => {
                self.apply_v2()?;
                self.apply_v3()?;
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            2 => {
                self.apply_v3()?;
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            3 => {
                self.apply_v4()?;
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            4 => {
                self.apply_v5()?;
                self.apply_v6()?;
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            5 => {
                self.apply_v6()?;
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            6 => {
                self.apply_v7()?;
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            7 => {
                self.apply_v8()?;
                self.apply_v9()?;
                self.apply_v10()
            }
            8 => {
                self.apply_v9()?;
                self.apply_v10()
            }
            9 => self.apply_v10(),
            SCHEMA_VERSION_V10
            | SCHEMA_VERSION_V11
            | PREVIOUS_SCHEMA_VERSION
            | CURRENT_SCHEMA_VERSION => Ok(()),
            _ => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    pub(super) fn apply_v11_if_needed(&mut self) -> Result<(), StateStoreError> {
        match self.schema_version() {
            SCHEMA_VERSION_V10 => {
                let transaction = self.connection.transaction().map_err(sqlite_error)?;
                transaction
                    .execute(V11_ACCOUNT_ROUTING_POLICY_TABLE_SQL, [])
                    .map_err(sqlite_error)?;
                transaction
                    .pragma_update(None, "user_version", SCHEMA_VERSION_V11)
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)
            }
            SCHEMA_VERSION_V11 | PREVIOUS_SCHEMA_VERSION | CURRENT_SCHEMA_VERSION => Ok(()),
            version => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    pub(super) fn apply_v12_if_needed(&mut self) -> Result<(), StateStoreError> {
        match self.schema_version() {
            SCHEMA_VERSION_V11 => {
                let transaction = self.connection.transaction().map_err(sqlite_error)?;
                rebuild_account_routing_policy_table_v12_sync(&transaction)?;
                transaction.commit().map_err(sqlite_error)
            }
            PREVIOUS_SCHEMA_VERSION | CURRENT_SCHEMA_VERSION => Ok(()),
            version => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    pub(super) fn apply_v13_if_needed(&mut self) -> Result<(), StateStoreError> {
        match self.schema_version() {
            PREVIOUS_SCHEMA_VERSION => {
                let transaction = self.connection.transaction().map_err(sqlite_error)?;
                transaction
                    .execute(V13_SESSION_ACCOUNT_AFFINITY_TABLE_SQL, [])
                    .map_err(sqlite_error)?;
                transaction
                    .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION)
                    .map_err(sqlite_error)?;
                transaction.commit().map_err(sqlite_error)
            }
            CURRENT_SCHEMA_VERSION => Ok(()),
            version => Err(StateStoreError::UnsupportedSchemaVersion { version }),
        }
    }

    pub(super) fn verify_account_routing_policy_schema(&self) -> Result<(), StateStoreError> {
        if !self.table_exists("account_routing_policies")? {
            return Err(StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "table",
                object_name: "account_routing_policies",
            });
        }
        for column_name in ["account_id", "weekly_quota_floor_basis_points"] {
            if !self.table_has_column("account_routing_policies", column_name)? {
                return Err(StateStoreError::MissingReadOnlySchemaObject {
                    object_kind: "column",
                    object_name: column_name,
                });
            }
        }
        Ok(())
    }

    /// Exercises sync-fixture rollback immediately before the v11 commit.
    #[cfg(test)]
    pub fn inject_v11_migration_rollback_for_test(
        database_path: &Path,
    ) -> Result<(), StateStoreError> {
        let mut connection = Connection::open(database_path).map_err(sqlite_error)?;
        let transaction = connection.transaction().map_err(sqlite_error)?;
        transaction
            .execute(V11_ACCOUNT_ROUTING_POLICY_TABLE_SQL, [])
            .map_err(sqlite_error)?;
        transaction
            .pragma_update(None, "user_version", SCHEMA_VERSION_V11)
            .map_err(sqlite_error)?;
        transaction.rollback().map_err(sqlite_error)?;
        Err(redacted_weekly_floor_sqlite_error(
            "injected migration failure",
        ))
    }

    /// Exercises sync-fixture rollback immediately before the v12 commit.
    #[cfg(test)]
    pub fn inject_v12_migration_rollback_for_test(
        database_path: &Path,
    ) -> Result<(), StateStoreError> {
        let mut connection = Connection::open(database_path).map_err(sqlite_error)?;
        let transaction = connection.transaction().map_err(sqlite_error)?;
        rebuild_account_routing_policy_table_v12_sync(&transaction)?;
        transaction.rollback().map_err(sqlite_error)?;
        Err(redacted_weekly_floor_sqlite_error(
            "injected migration failure",
        ))
    }

    pub(super) fn ensure_async_read_only_schema(&self) -> Result<(), StateStoreError> {
        for statement in ASYNC_QUOTA_HISTORY_SCHEMA_STATEMENTS
            .iter()
            .chain(ASYNC_CREDIT_USAGE_SCHEMA_STATEMENTS)
            .chain(ASYNC_ACTIVE_CLIENT_SCHEMA_STATEMENTS)
            .chain(ASYNC_ROUTE_BAND_ACCOUNT_STATE_SCHEMA_STATEMENTS)
            .chain(ASYNC_ACTIVE_SESSION_HISTORY_TABLE_STATEMENTS)
            .chain(ASYNC_ACTIVE_SESSION_HISTORY_INDEX_STATEMENTS)
        {
            self.connection
                .execute_batch(statement)
                .map_err(sqlite_error)?;
        }

        Ok(())
    }

    pub(super) fn table_exists(&self, table_name: &str) -> Result<bool, StateStoreError> {
        self.connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1
                )",
                params![table_name],
                |row| row.get::<_, i64>(0),
            )
            .map(|exists| exists != 0)
            .map_err(sqlite_error)
    }

    pub(super) fn table_has_column(
        &self,
        table_name: &str,
        column_name: &str,
    ) -> Result<bool, StateStoreError> {
        let query = format!("PRAGMA table_info({})", sqlite_identifier(table_name));
        let mut statement = self.connection.prepare(&query).map_err(sqlite_error)?;
        let mut rows = statement.query([]).map_err(sqlite_error)?;
        while let Some(row) = rows.next().map_err(sqlite_error)? {
            let name: String = row.get("name").map_err(sqlite_error)?;
            if name == column_name {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

#[cfg(any(test, feature = "sync-rusqlite-fixtures"))]
fn sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}
