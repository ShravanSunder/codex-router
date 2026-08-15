//! SQLite-backed metadata boundary for codex-router.

pub mod account;
pub mod account_routing_policy;
pub mod affinity_owner;
pub mod quota_snapshot;
pub mod repositories;
pub mod selection_projection;
pub mod session_account_affinity;
pub mod sqlite;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-state"
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use codex_router_core::affinity::AffinityKeyHash;
    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::AffinityKey;
    use codex_router_core::ids::ReservationId;
    use codex_router_core::routes::RouteBand;
    use codex_router_selection::burn_down::V1_WEEKLY_WINDOW_SECONDS;
    use codex_router_selection::run_rate::QuotaRunRateConfidence;
    use rusqlite::Connection;
    use sqlx::Row;

    use super::package_name;
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
    use crate::quota_snapshot::QuotaRefreshStatusSource;
    use crate::quota_snapshot::QuotaSnapshotSource;
    use crate::quota_snapshot::SelectorQuotaWindowStatus;
    use crate::repositories::AccountStateRepository;
    use crate::repositories::AffinityRepository;
    use crate::repositories::QuotaSnapshotRepository;
    use crate::repositories::SelectorQuotaRepository;
    use crate::selection_projection::project_route_band_selection_inputs;
    use crate::selection_projection::project_route_band_selection_inputs_with_active_counts;
    use crate::session_account_affinity::SessionAccountAffinity;
    use crate::sqlite::AsyncAffinityRepository;
    use crate::sqlite::AsyncQuotaExhaustionRepository;
    use crate::sqlite::AsyncQuotaHistoryRepository;
    use crate::sqlite::AsyncSelectorQuotaRepository;
    use crate::sqlite::AsyncSessionAccountAffinityRepository;
    use crate::sqlite::AsyncSqliteStateStore;
    use crate::sqlite::AsyncWeeklyQuotaFloorMutationStore;
    use crate::sqlite::SqliteStateStore;
    use crate::sqlite::StateStoreError;
    use crate::sqlite::WeeklyQuotaFloorMutationResult;

    fn expect_error<T, E>(result: Result<T, E>, context: &'static str) -> E {
        match result {
            Ok(_) => panic!("{context}"),
            Err(error) => error,
        }
    }

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn convert_current_fixture_to_v10(database_path: &Path) {
        let connection = Connection::open(database_path)
            .unwrap_or_else(|error| panic!("fixture should reopen for v10 conversion: {error}"));
        connection
            .execute_batch(
                "DROP TABLE account_routing_policies;
                 DROP TABLE session_account_affinities;
                 PRAGMA user_version = 10;",
            )
            .unwrap_or_else(|error| panic!("fixture should convert to v10: {error}"));
    }

    fn convert_current_fixture_to_v11(database_path: &Path) {
        let connection = Connection::open(database_path)
            .unwrap_or_else(|error| panic!("fixture should reopen for v11 conversion: {error}"));
        connection
            .execute_batch(
                "ALTER TABLE account_routing_policies RENAME TO account_routing_policies_current;
                 CREATE TABLE account_routing_policies (
                    account_id TEXT PRIMARY KEY NOT NULL,
                    weekly_quota_floor_basis_points INTEGER NOT NULL
                        CHECK (
                            weekly_quota_floor_basis_points BETWEEN 100 AND 1000
                            AND weekly_quota_floor_basis_points % 100 = 0
                        )
                 );
                 INSERT INTO account_routing_policies
                    SELECT * FROM account_routing_policies_current;
                 DROP TABLE account_routing_policies_current;
                 DROP TABLE session_account_affinities;
                 PRAGMA user_version = 11;",
            )
            .unwrap_or_else(|error| panic!("fixture should convert to v11: {error}"));
    }

    fn convert_current_fixture_to_v12(database_path: &Path) {
        let connection = Connection::open(database_path)
            .unwrap_or_else(|error| panic!("fixture should reopen for v12 conversion: {error}"));
        connection
            .execute_batch(
                "DROP TABLE session_account_affinities;
                 PRAGMA user_version = 12;",
            )
            .unwrap_or_else(|error| panic!("fixture should convert to v12: {error}"));
    }

    fn assert_intact_v11_policy_database(
        database_path: &Path,
        expected_account_id: &str,
        expected_basis_points: i64,
    ) {
        let connection = Connection::open(database_path).expect("v11 fixture should reopen");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version should read");
        assert_eq!(version, 11);
        let stored_basis_points: i64 = connection
            .query_row(
                "SELECT weekly_quota_floor_basis_points
                   FROM account_routing_policies
                  WHERE account_id = ?1",
                [expected_account_id],
                |row| row.get(0),
            )
            .expect("preserved policy should read");
        assert_eq!(stored_basis_points, expected_basis_points);
        let table_sql: String = connection
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'account_routing_policies'",
                [],
                |row| row.get(0),
            )
            .expect("v11 policy schema should read");
        assert!(table_sql.contains("BETWEEN 100 AND 1000"));
        let replacement_exists: i64 = connection
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'account_routing_policies_v12'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("replacement-table state should read");
        assert_eq!(replacement_exists, 0);
        let integrity: String = connection
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .expect("integrity check should run");
        assert_eq!(integrity, "ok");
    }

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-state");
    }

    #[tokio::test]
    async fn session_account_affinity_v12_migrates_to_v13() {
        let temp_dir = TestTempDir::new("session_account_affinity_v12_to_v13");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("current fixture should open");
        store.close().await.expect("fixture should close");
        convert_current_fixture_to_v12(&database_path);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("v12 database should migrate");

        assert_eq!(migrated.schema_version().await, Ok(13));
        migrated.close().await.expect("migrated store should close");
        let raw = Connection::open(&database_path).expect("migrated database should inspect");
        let mut statement = raw
            .prepare("PRAGMA table_info(session_account_affinities)")
            .expect("session affinity schema should prepare");
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .expect("session affinity columns should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("session affinity columns should load");
        assert_eq!(
            columns,
            vec!["session_id", "account_id", "last_seen_unix_seconds"]
        );
    }

    #[tokio::test]
    async fn session_account_affinity_upsert_replaces_account_and_last_seen() {
        let temp_dir = TestTempDir::new("session_account_affinity_upsert");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state store should open");
        let first = SessionAccountAffinity::new("session-123", account_id("acct_first"), 1_000);
        let replacement =
            SessionAccountAffinity::new("session-123", account_id("acct_replacement"), 1_100);

        AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(&store, &first)
            .await
            .expect("first mapping should persist");
        AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(
            &store,
            &replacement,
        )
        .await
        .expect("replacement mapping should persist");

        assert_eq!(
            AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                &store,
                "session-123",
            )
            .await,
            Ok(Some(replacement))
        );
    }

    #[tokio::test]
    async fn session_account_affinity_purge_deletes_only_rows_before_cutoff() {
        let temp_dir = TestTempDir::new("session_account_affinity_purge");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state store should open");
        for affinity in [
            SessionAccountAffinity::new("session-old", account_id("acct_old"), 999),
            SessionAccountAffinity::new("session-cutoff", account_id("acct_cutoff"), 1_000),
            SessionAccountAffinity::new("session-fresh", account_id("acct_fresh"), 1_001),
        ] {
            AsyncSessionAccountAffinityRepository::upsert_session_account_affinity(
                &store, &affinity,
            )
            .await
            .expect("session affinity should persist");
        }

        store
            .purge_session_account_affinities_before(1_000)
            .await
            .expect("old session affinities should purge");

        assert_eq!(
            AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                &store,
                "session-old",
            )
            .await,
            Ok(None)
        );
        for retained_session_id in ["session-cutoff", "session-fresh"] {
            assert!(
                AsyncSessionAccountAffinityRepository::load_session_account_affinity(
                    &store,
                    retained_session_id,
                )
                .await
                .expect("retained affinity should load")
                .is_some()
            );
        }
        assert_eq!(store.schema_version().await, Ok(13));
        store.close().await.expect("state store should close");

        let raw = Connection::open(&database_path).expect("database should inspect");
        let index_count: i64 = raw
            .query_row(
                "SELECT COUNT(*)
                   FROM pragma_index_list('session_account_affinities')
                  WHERE name = 'session_account_affinities_last_seen_lookup'",
                [],
                |row| row.get(0),
            )
            .expect("session affinity indexes should query");
        assert_eq!(index_count, 1);
    }

    #[tokio::test]
    async fn existing_v13_database_adds_session_affinity_index_without_version_change() {
        let temp_dir = TestTempDir::new("session_account_affinity_v13_index_upgrade");
        let database_path = temp_dir.path().join("state.sqlite");
        let initial_store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("initial v13 store should open");
        initial_store
            .close()
            .await
            .expect("initial store should close");
        let existing_v13 = Connection::open(&database_path).expect("v13 database should open");
        existing_v13
            .execute("DROP INDEX session_account_affinities_last_seen_lookup", [])
            .expect("test fixture should remove the post-v13 index");
        assert_eq!(
            existing_v13
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .expect("v13 fixture version should query"),
            13
        );
        drop(existing_v13);

        let reopened = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("existing v13 database should reopen");

        assert_eq!(reopened.schema_version().await, Ok(13));
        reopened.close().await.expect("reopened store should close");
        let inspected = Connection::open(&database_path).expect("reopened database should inspect");
        let index_count: i64 = inspected
            .query_row(
                "SELECT COUNT(*)
                   FROM pragma_index_list('session_account_affinities')
                  WHERE name = 'session_account_affinities_last_seen_lookup'",
                [],
                |row| row.get(0),
            )
            .expect("session affinity indexes should query");
        assert_eq!(index_count, 1);
    }

    #[test]
    fn weekly_floor_accepts_only_integer_percent_basis_points() {
        for basis_points in [100_u16, 200, 500, 1_000, 1_500] {
            assert_eq!(
                WeeklyQuotaFloorBasisPoints::new(basis_points)
                    .map(WeeklyQuotaFloorBasisPoints::basis_points),
                Ok(basis_points)
            );
        }
        for basis_points in [0_u16, 1, 99, 101, 999, 1_501, 1_600] {
            assert!(WeeklyQuotaFloorBasisPoints::new(basis_points).is_err());
        }
    }

    #[tokio::test]
    async fn v10_migrates_through_v11_to_v12_and_exposes_policy_read_only() {
        let temp_dir = TestTempDir::new("v10_to_v12_policy_migration");
        let database_path = temp_dir.path().join("state.sqlite");
        let v10 = SqliteStateStore::open(&database_path)
            .unwrap_or_else(|error| panic!("v10 fixture should open: {error}"));
        let account = AccountRecord::new(
            account_id("acct_v11_preserved"),
            "preserved",
            AccountStatus::Enabled,
        );
        v10.upsert_account(&account)
            .unwrap_or_else(|error| panic!("v10 account should persist: {error}"));
        drop(v10);
        let raw = Connection::open(&database_path).expect("fixture should reopen for seeding");
        raw.execute_batch(
            "INSERT INTO quota_snapshots VALUES (
                'acct_v11_preserved', 'mock_endpoint', 100, 'responses', 5000,
                1000, NULL, 0
             );
             INSERT INTO selector_quota_windows VALUES (
                'acct_v11_preserved', 'responses', 604800, 'eligible', 5000,
                1000, 1, 100
             );
             INSERT INTO active_client_leases VALUES (
                'responses', 'process', 'reservation', 'acct_v11_preserved', 100, 0
             );
             INSERT INTO active_session_events (
                account_id, route_band, process_run_id, logical_session_id,
                reservation_id, event_kind, event_unix_seconds,
                session_started_unix_seconds, session_ended_unix_seconds,
                transport_kind
             ) VALUES (
                'acct_v11_preserved', 'responses', 'process', 'session',
                'reservation', 'acquired', 100, 100, NULL, 'websocket'
             );
             INSERT INTO active_session_rollups VALUES (
                'acct_v11_preserved', 'responses', 0, 300, 100, 1, 0, 0
             );
             CREATE TABLE unrelated_v10_sentinel (value TEXT NOT NULL);
             INSERT INTO unrelated_v10_sentinel VALUES ('preserved');",
        )
        .expect("canonical v10 rows should seed");
        drop(raw);
        convert_current_fixture_to_v10(&database_path);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("v10 database should migrate: {error}"));
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(migrated.list_accounts().await, Ok(vec![account]));
        assert_eq!(
            migrated.list_account_routing_policies().await,
            Ok(Vec::new())
        );
        migrated.close().await.expect("migrated store should close");

        let raw = Connection::open(&database_path).expect("migrated database should inspect");
        for table_name in [
            "quota_snapshots",
            "selector_quota_windows",
            "active_client_leases",
            "active_session_events",
            "active_session_rollups",
            "unrelated_v10_sentinel",
        ] {
            let count: i64 = raw
                .query_row(&format!("SELECT COUNT(*) FROM {table_name}"), [], |row| {
                    row.get(0)
                })
                .unwrap_or_else(|error| panic!("{table_name} should survive: {error}"));
            assert_eq!(count, 1, "{table_name} row should survive migration");
        }
        let integrity: String = raw
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .expect("integrity check should run");
        assert_eq!(integrity, "ok");
        assert!(
            raw.execute(
                "INSERT INTO account_routing_policies VALUES ('acct_v11_preserved', 50)",
                [],
            )
            .is_err()
        );
        assert!(
            raw.execute(
                "INSERT INTO account_routing_policies VALUES ('acct_v11_preserved', 1501)",
                [],
            )
            .is_err()
        );
        drop(raw);

        let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .unwrap_or_else(|error| panic!("v12 should reopen read-only: {error}"));
        assert_eq!(
            read_only.list_account_routing_policies().await,
            Ok(Vec::new())
        );
    }

    #[tokio::test]
    async fn v12_migration_preserves_v11_policy_and_expands_integer_percent_check() {
        let temp_dir = TestTempDir::new("v11_to_v12_policy_migration");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("current fixture should open");
        let account_id = account_id("acct_v12_preserved");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "v12-preserved",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        store.close().await.expect("fixture should close");
        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("current mutation store should open");
        let preserved_floor = WeeklyQuotaFloorBasisPoints::new(1_000).expect("valid v11 floor");
        mutation
            .set_weekly_quota_floor_by_label("v12-preserved", Some(preserved_floor))
            .await
            .expect("v11-compatible floor should persist");
        mutation.close().await;
        convert_current_fixture_to_v11(&database_path);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("v11 database should migrate to v12");
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(
            migrated.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(
                account_id.clone(),
                preserved_floor
            )])
        );
        migrated.close().await.expect("migrated store should close");

        let raw = Connection::open(&database_path).expect("v12 database should inspect");
        let table_sql: String = raw
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'account_routing_policies'",
                [],
                |row| row.get(0),
            )
            .expect("policy table SQL should read");
        assert!(table_sql.contains("BETWEEN 100 AND 1500"));
        assert_eq!(
            raw.execute(
                "INSERT INTO account_routing_policies VALUES ('acct_v12_fifteen', 1500)",
                [],
            ),
            Ok(1)
        );
        for invalid_basis_points in [1_501, 1_550, 1_600] {
            assert!(
                raw.execute(
                    "INSERT INTO account_routing_policies VALUES (?1, ?2)",
                    (
                        format!("acct_invalid_{invalid_basis_points}"),
                        invalid_basis_points,
                    ),
                )
                .is_err(),
                "{invalid_basis_points} basis points must be rejected"
            );
        }
        let integrity: String = raw
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .expect("integrity check should run");
        assert_eq!(integrity, "ok");
    }

    #[tokio::test]
    async fn v10_to_v11_migration_rolls_back_atomically_and_retries_to_v12() {
        let temp_dir = TestTempDir::new("v11_async_rollback_retry");
        let database_path = temp_dir.path().join("state.sqlite");
        let account = AccountRecord::new(
            account_id("acct_v11_async_rollback"),
            "rollback",
            AccountStatus::Enabled,
        );
        let store = SqliteStateStore::open(&database_path).expect("fixture should open");
        store
            .upsert_account(&account)
            .expect("account should persist");
        drop(store);
        convert_current_fixture_to_v10(&database_path);

        let error = AsyncSqliteStateStore::inject_v11_migration_rollback_for_test(&database_path)
            .await
            .expect_err("migration failure should be injected");
        assert!(!error.to_string().contains(account.account_id().as_str()));
        let connection = Connection::open(&database_path).expect("fixture should reopen");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version should read");
        assert_eq!(version, 10);
        assert!(
            connection
                .prepare("SELECT 1 FROM account_routing_policies")
                .is_err()
        );
        drop(connection);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("migration retry should succeed");
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(migrated.list_accounts().await, Ok(vec![account]));
    }

    #[test]
    fn v10_to_v11_sync_migration_rolls_back_atomically_and_retries_to_v12() {
        let temp_dir = TestTempDir::new("v11_sync_rollback_retry");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = SqliteStateStore::open(&database_path).expect("fixture should open");
        drop(store);
        convert_current_fixture_to_v10(&database_path);

        SqliteStateStore::inject_v11_migration_rollback_for_test(&database_path)
            .expect_err("sync migration failure should be injected");
        let connection = Connection::open(&database_path).expect("fixture should reopen");
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version should read");
        assert_eq!(version, 10);
        assert!(
            connection
                .prepare("SELECT 1 FROM account_routing_policies")
                .is_err()
        );
        drop(connection);

        let migrated =
            SqliteStateStore::open(&database_path).expect("sync migration retry should succeed");
        assert_eq!(migrated.schema_version(), 13);
    }

    #[tokio::test]
    async fn v12_async_migration_rolls_back_to_intact_v11_and_retries() {
        let temp_dir = TestTempDir::new("v12_async_rollback_retry");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("current fixture should open");
        let account_id = account_id("acct_v12_async_rollback");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "v12-async-rollback",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        store.close().await.expect("fixture should close");
        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("current mutation store should open");
        let floor = WeeklyQuotaFloorBasisPoints::new(900).expect("valid v11 floor");
        mutation
            .set_weekly_quota_floor_by_label("v12-async-rollback", Some(floor))
            .await
            .expect("policy should persist");
        mutation.close().await;
        convert_current_fixture_to_v11(&database_path);

        AsyncSqliteStateStore::inject_v12_migration_rollback_for_test(&database_path)
            .await
            .expect_err("v12 migration failure should be injected");
        assert_intact_v11_policy_database(&database_path, account_id.as_str(), 900);

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("v12 migration retry should succeed");
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(
            migrated.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(account_id, floor)])
        );
    }

    #[test]
    fn v12_sync_migration_rolls_back_to_intact_v11_and_retries() {
        let temp_dir = TestTempDir::new("v12_sync_rollback_retry");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = SqliteStateStore::open(&database_path).expect("current fixture should open");
        let account_id = account_id("acct_v12_sync_rollback");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "v12-sync-rollback",
                AccountStatus::Enabled,
            ))
            .expect("account should persist");
        drop(store);
        let raw = Connection::open(&database_path).expect("fixture should reopen");
        raw.execute(
            "INSERT INTO account_routing_policies VALUES (?1, 900)",
            [account_id.as_str()],
        )
        .expect("policy should persist");
        drop(raw);
        convert_current_fixture_to_v11(&database_path);

        SqliteStateStore::inject_v12_migration_rollback_for_test(&database_path)
            .expect_err("v12 migration failure should be injected");
        assert_intact_v11_policy_database(&database_path, account_id.as_str(), 900);

        let migrated = SqliteStateStore::open(&database_path)
            .expect("sync v12 migration retry should succeed");
        assert_eq!(migrated.schema_version(), 13);
        drop(migrated);
        let raw = Connection::open(&database_path).expect("migrated database should reopen");
        let preserved_floor: i64 = raw
            .query_row(
                "SELECT weekly_quota_floor_basis_points FROM account_routing_policies WHERE account_id = ?1",
                [account_id.as_str()],
                |row| row.get(0),
            )
            .expect("sync migration should preserve policy");
        assert_eq!(preserved_floor, 900);
    }

    #[tokio::test]
    async fn mutation_only_store_requires_v12_and_roundtrips_set_then_delete() {
        let temp_dir = TestTempDir::new("weekly_floor_mutation");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("state should open: {error}"));
        let account_id = account_id("acct_weekly_floor");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "weekly-floor",
                AccountStatus::Enabled,
            ))
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store.close().await.expect("state should close");

        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("v12 mutation store should open: {error}"));
        let floor = WeeklyQuotaFloorBasisPoints::new(500)
            .unwrap_or_else(|error| panic!("floor should validate: {error}"));
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_account_id(&account_id, Some(floor))
                .await,
            Ok(WeeklyQuotaFloorMutationResult::Enabled(
                AccountRoutingPolicy::new(account_id.clone(), floor)
            ))
        );
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_account_id(&account_id, None)
                .await,
            Ok(WeeklyQuotaFloorMutationResult::Disabled)
        );
        let missing_account_id =
            AccountId::new("acct_weekly_floor_missing").expect("valid missing account id");
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_account_id(&missing_account_id, Some(floor))
                .await,
            Err(StateStoreError::WeeklyQuotaFloorAccountNotFound)
        );
        mutation.close().await;

        let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .unwrap_or_else(|error| panic!("state should reopen: {error}"));
        assert_eq!(
            read_only.list_account_routing_policies().await,
            Ok(Vec::new())
        );
    }

    #[tokio::test]
    async fn mutation_only_store_rejects_v10_and_v11_without_migrating() {
        let temp_dir = TestTempDir::new("weekly_floor_requires_v12");
        let database_path = temp_dir.path().join("state.sqlite");
        let raw = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("v10 fixture should open: {error}"));
        raw.execute_batch(
            "CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
             );
             PRAGMA user_version = 10;",
        )
        .unwrap_or_else(|error| panic!("v10 fixture should initialize: {error}"));
        drop(raw);

        let error = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect_err("mutation open must reject v10");
        assert_eq!(
            error,
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
        );

        let raw = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("v10 fixture should reopen: {error}"));
        let version: i64 = raw
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or_else(|error| panic!("version should read: {error}"));
        assert_eq!(version, 10);
        assert!(
            raw.prepare("SELECT 1 FROM account_routing_policies")
                .is_err()
        );
        raw.execute_batch(
            "CREATE TABLE account_routing_policies (
                account_id TEXT PRIMARY KEY NOT NULL,
                weekly_quota_floor_basis_points INTEGER NOT NULL
                    CHECK (
                        weekly_quota_floor_basis_points BETWEEN 100 AND 1000
                        AND weekly_quota_floor_basis_points % 100 = 0
                    )
             );
             PRAGMA user_version = 11;",
        )
        .expect("v11 policy schema should initialize");
        drop(raw);

        let error = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect_err("mutation open must reject v11");
        assert_eq!(
            error,
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
        );
        let raw = Connection::open(&database_path)
            .unwrap_or_else(|error| panic!("v11 fixture should reopen: {error}"));
        let version: i64 = raw
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap_or_else(|error| panic!("version should read: {error}"));
        assert_eq!(version, 11);
    }

    #[tokio::test]
    async fn read_only_open_rejects_v11_without_migrating_policy_state() {
        let temp_dir = TestTempDir::new("read_only_v11_no_migration");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("current state should open");
        let account_id = account_id("acct_read_only_v11");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "read-only-v11",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        store.close().await.expect("state should close");
        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("current mutation store should open");
        mutation
            .set_weekly_quota_floor_by_account_id(
                &account_id,
                Some(WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor")),
            )
            .await
            .expect("policy should persist");
        mutation.close().await;
        convert_current_fixture_to_v11(&database_path);

        assert_eq!(
            AsyncSqliteStateStore::open_read_only(&database_path)
                .await
                .expect_err("read-only open must reject v11 without migrating"),
            StateStoreError::UnsupportedSchemaVersion { version: 11 }
        );
        assert_intact_v11_policy_database(&database_path, account_id.as_str(), 800);
    }

    #[tokio::test]
    async fn every_v12_open_rejects_malformed_policy_columns() {
        let temp_dir = TestTempDir::new("v12_malformed_policy_columns");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("valid v12 should open");
        store.close().await.expect("valid v12 should close");
        let raw = Connection::open(&database_path).expect("fixture should reopen");
        raw.execute_batch(
            "DROP TABLE account_routing_policies;
             CREATE TABLE account_routing_policies (account_id TEXT PRIMARY KEY NOT NULL);",
        )
        .expect("malformed policy table should install");
        drop(raw);

        for error in [
            AsyncSqliteStateStore::open(&database_path)
                .await
                .expect_err("async writable open must reject malformed policy schema"),
            AsyncSqliteStateStore::open_read_only(&database_path)
                .await
                .expect_err("read-only open must reject malformed policy schema"),
        ] {
            assert_eq!(
                error,
                StateStoreError::MissingReadOnlySchemaObject {
                    object_kind: "column",
                    object_name: "weekly_quota_floor_basis_points",
                }
            );
        }
        assert_eq!(
            AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
                .await
                .expect_err("mutation open must reject malformed policy schema"),
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
        );
        assert_eq!(
            SqliteStateStore::open(&database_path)
                .expect_err("sync writable open must reject malformed policy schema"),
            StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "column",
                object_name: "weekly_quota_floor_basis_points",
            }
        );

        let raw = Connection::open(&database_path).expect("fixture should reopen");
        raw.execute_batch("DROP TABLE account_routing_policies;")
            .expect("policy table should drop");
        drop(raw);
        for error in [
            AsyncSqliteStateStore::open(&database_path)
                .await
                .expect_err("async writable open must reject missing policy table"),
            AsyncSqliteStateStore::open_read_only(&database_path)
                .await
                .expect_err("read-only open must reject missing policy table"),
        ] {
            assert_eq!(
                error,
                StateStoreError::MissingReadOnlySchemaObject {
                    object_kind: "table",
                    object_name: "account_routing_policies",
                }
            );
        }
        assert_eq!(
            AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
                .await
                .expect_err("mutation open must reject missing policy table"),
            StateStoreError::WeeklyQuotaFloorSchemaUpgradeRequired
        );
        assert_eq!(
            SqliteStateStore::open(&database_path)
                .expect_err("sync writable open must reject missing policy table"),
            StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "table",
                object_name: "account_routing_policies",
            }
        );
    }

    #[tokio::test]
    async fn production_async_v12_migration_contention_is_atomic_and_retryable() {
        let temp_dir = TestTempDir::new("v12_production_migration_contention");
        let database_path = temp_dir.path().join("state.sqlite");
        let account = AccountRecord::new(
            account_id("acct_migration_contention"),
            "migration-contention",
            AccountStatus::Enabled,
        );
        let store = SqliteStateStore::open(&database_path).expect("fixture should open");
        store
            .upsert_account(&account)
            .expect("account should persist");
        drop(store);
        let raw = Connection::open(&database_path).expect("fixture should reopen");
        raw.execute(
            "INSERT INTO account_routing_policies VALUES (?1, 800)",
            [account.account_id().as_str()],
        )
        .expect("v11-compatible policy should persist");
        drop(raw);
        convert_current_fixture_to_v11(&database_path);

        let lock = Connection::open(&database_path).expect("lock connection should open");
        lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .expect("writer lock should be held");
        let migration_attempt = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            AsyncSqliteStateStore::open(&database_path),
        )
        .await;
        match migration_attempt {
            Ok(Err(StateStoreError::Sqlite { message })) => assert!(
                message.contains("locked") || message.contains("busy"),
                "held-writer migration should return a lock error, got: {message}"
            ),
            Ok(Err(error)) => panic!("held-writer migration returned the wrong error: {error}"),
            Ok(Ok(_)) => panic!("migration must not commit beneath a held writer"),
            Err(_) => panic!("production migration must return before the test timeout"),
        }
        let version: i64 = lock
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version should read under lock");
        assert_eq!(version, 11);
        let preserved_floor: i64 = lock
            .query_row(
                "SELECT weekly_quota_floor_basis_points FROM account_routing_policies WHERE account_id = ?1",
                [account.account_id().as_str()],
                |row| row.get(0),
            )
            .expect("v11 policy should remain readable");
        assert_eq!(preserved_floor, 800);
        let replacement_exists: i64 = lock
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM sqlite_master
                     WHERE type = 'table' AND name = 'account_routing_policies_v12'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("replacement-table state should read");
        assert_eq!(replacement_exists, 0);
        let preserved_accounts: i64 = lock
            .query_row("SELECT COUNT(*) FROM accounts", [], |row| row.get(0))
            .expect("preserved account count should read");
        assert_eq!(preserved_accounts, 1);
        lock.execute_batch("ROLLBACK;")
            .expect("lock should release");

        let migrated = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("production migration retry should succeed");
        assert_eq!(migrated.schema_version().await, Ok(13));
        assert_eq!(migrated.list_accounts().await, Ok(vec![account.clone()]));
        assert_eq!(
            migrated.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(
                account.account_id().clone(),
                WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor")
            )])
        );
        migrated.close().await.expect("migrated store should close");
        let raw = Connection::open(&database_path).expect("migrated DB should inspect");
        let integrity: String = raw
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .expect("integrity check should run");
        assert_eq!(integrity, "ok");
    }

    #[tokio::test]
    async fn policy_bulk_read_is_independent_and_unknown_account_is_redacted() {
        let temp_dir = TestTempDir::new("weekly_floor_bulk_read");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("state should open: {error}"));
        let account_a = account_id("acct_policy_a");
        let account_b = account_id("acct_policy_b");
        for (account_id, label) in [(&account_a, "policy-a"), (&account_b, "policy-b")] {
            store
                .upsert_account(&AccountRecord::new(
                    account_id.clone(),
                    label,
                    AccountStatus::Enabled,
                ))
                .await
                .unwrap_or_else(|error| panic!("account should persist: {error}"));
        }
        store.close().await.expect("state should close");

        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("mutation store should open: {error}"));
        let floor_a = WeeklyQuotaFloorBasisPoints::new(300).expect("valid floor");
        let floor_b = WeeklyQuotaFloorBasisPoints::new(900).expect("valid floor");
        mutation
            .set_weekly_quota_floor_by_label("policy-a", Some(floor_a))
            .await
            .expect("policy A should persist");
        mutation
            .set_weekly_quota_floor_by_label("policy-b", Some(floor_b))
            .await
            .expect("policy B should persist");
        let missing_label = "sensitive-missing-label";
        let error = mutation
            .set_weekly_quota_floor_by_label(missing_label, Some(floor_a))
            .await
            .expect_err("unknown account must fail");
        assert_eq!(error, StateStoreError::WeeklyQuotaFloorAccountNotFound);
        assert!(!error.to_string().contains(missing_label));
        mutation.close().await;

        let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("state should reopen read-only");
        assert_eq!(
            read_only.list_account_routing_policies().await,
            Ok(vec![
                AccountRoutingPolicy::new(account_a, floor_a),
                AccountRoutingPolicy::new(account_b, floor_b),
            ])
        );
    }

    #[tokio::test]
    async fn weekly_floor_label_resolution_rejects_duplicate_exact_labels_without_mutation() {
        let temp_dir = TestTempDir::new("weekly_floor_duplicate_label");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        for account_id_value in ["acct_duplicate_a", "acct_duplicate_b"] {
            state
                .upsert_account(&AccountRecord::new(
                    account_id(account_id_value),
                    "duplicate-label",
                    AccountStatus::Enabled,
                ))
                .await
                .expect("duplicate-label account should persist");
        }
        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("mutation store should open");
        let error = mutation
            .set_weekly_quota_floor_by_label(
                "duplicate-label",
                Some(WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor")),
            )
            .await
            .expect_err("duplicate exact labels must fail");
        assert_eq!(
            error,
            StateStoreError::WeeklyQuotaFloorAccountLabelAmbiguous
        );
        assert!(!error.to_string().contains("duplicate-label"));
        assert_eq!(state.list_account_routing_policies().await, Ok(Vec::new()));
        let target_account_id = account_id("acct_duplicate_b");
        let target_floor = WeeklyQuotaFloorBasisPoints::new(500).expect("valid floor");
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_account_id(&target_account_id, Some(target_floor))
                .await,
            Ok(WeeklyQuotaFloorMutationResult::Enabled(
                AccountRoutingPolicy::new(target_account_id.clone(), target_floor)
            ))
        );
        assert_eq!(
            state.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(
                target_account_id,
                target_floor
            )])
        );
        mutation.close().await;
    }

    #[tokio::test]
    async fn weekly_floor_transaction_resolves_current_account_label_metadata() {
        let temp_dir = TestTempDir::new("weekly_floor_current_label");
        let database_path = temp_dir.path().join("state.sqlite");
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        let account_id = account_id("acct_current_label");
        state
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "old-label",
                AccountStatus::Enabled,
            ))
            .await
            .expect("old label should persist");
        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("mutation store should open");

        state
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "new-label",
                AccountStatus::Enabled,
            ))
            .await
            .expect("new label should replace old metadata");
        let floor = WeeklyQuotaFloorBasisPoints::new(800).expect("valid floor");
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_label("old-label", Some(floor))
                .await,
            Err(StateStoreError::WeeklyQuotaFloorAccountNotFound)
        );
        assert_eq!(
            mutation
                .set_weekly_quota_floor_by_label("new-label", Some(floor))
                .await,
            Ok(WeeklyQuotaFloorMutationResult::Enabled(
                AccountRoutingPolicy::new(account_id, floor)
            ))
        );
        mutation.close().await;
    }

    #[tokio::test]
    async fn weekly_floor_mutation_busy_retry_is_bounded_and_preserves_value() {
        let temp_dir = TestTempDir::new("weekly_floor_busy");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        let account_id = account_id("acct_policy_busy");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "busy-policy",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        store.close().await.expect("state should close");

        let mutation = AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
            .await
            .expect("mutation store should open");
        let old_floor = WeeklyQuotaFloorBasisPoints::new(400).expect("valid old floor");
        mutation
            .set_weekly_quota_floor_by_label("busy-policy", Some(old_floor))
            .await
            .expect("old floor should persist");

        let lock = Connection::open(&database_path).expect("lock connection should open");
        lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .expect("writer lock should be held");
        let concurrent_reader = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("WAL reader should remain responsive during a held writer");
        assert_eq!(
            concurrent_reader.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(
                account_id.clone(),
                old_floor
            )])
        );
        concurrent_reader
            .close()
            .await
            .expect("reader should close");
        let started_at = std::time::Instant::now();
        let error = mutation
            .set_weekly_quota_floor_by_account_id(
                &account_id,
                Some(WeeklyQuotaFloorBasisPoints::new(700).expect("valid new floor")),
            )
            .await
            .expect_err("held writer must exhaust retries");
        let elapsed = started_at.elapsed();
        assert_eq!(error, StateStoreError::WeeklyQuotaFloorDatabaseBusy);
        assert!(elapsed >= std::time::Duration::from_millis(150));
        assert!(elapsed < std::time::Duration::from_millis(250));
        lock.execute_batch("ROLLBACK;")
            .expect("lock should release");
        mutation.close().await;

        let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("state should reopen read-only");
        assert_eq!(
            read_only.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(account_id, old_floor)])
        );
    }

    #[tokio::test]
    async fn weekly_floor_mutation_commits_after_held_writer_releases() {
        let temp_dir = TestTempDir::new("weekly_floor_busy_then_release");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("state should open");
        let account_id = account_id("acct_policy_busy_then_release");
        store
            .upsert_account(&AccountRecord::new(
                account_id.clone(),
                "busy-then-release",
                AccountStatus::Enabled,
            ))
            .await
            .expect("account should persist");
        store.close().await.expect("state should close");

        let lock = Connection::open(&database_path).expect("lock connection should open");
        lock.execute_batch("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;")
            .expect("writer lock should be held");
        let mutation = Arc::new(
            AsyncWeeklyQuotaFloorMutationStore::open(&database_path)
                .await
                .expect("mutation store should open"),
        );
        let committed_floor = WeeklyQuotaFloorBasisPoints::new(600).expect("valid floor");
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let mutation_task = {
            let mutation = Arc::clone(&mutation);
            tokio::spawn(async move {
                started_sender
                    .send(())
                    .expect("test should observe mutation start");
                mutation
                    .set_weekly_quota_floor_by_label("busy-then-release", Some(committed_floor))
                    .await
            })
        };
        started_receiver
            .await
            .expect("mutation task should establish its first attempt");
        tokio::task::yield_now().await;
        assert!(
            !mutation_task.is_finished(),
            "held writer should leave the setter pending for a retry"
        );
        lock.execute_batch("ROLLBACK;")
            .expect("lock should release");

        assert_eq!(
            mutation_task.await.expect("mutation task should join"),
            Ok(WeeklyQuotaFloorMutationResult::Enabled(
                AccountRoutingPolicy::new(account_id.clone(), committed_floor)
            ))
        );
        let mutation = Arc::into_inner(mutation).expect("mutation task should release store");
        mutation.close().await;

        let read_only = AsyncSqliteStateStore::open_read_only(&database_path)
            .await
            .expect("state should reopen read-only");
        assert_eq!(
            read_only.list_account_routing_policies().await,
            Ok(vec![AccountRoutingPolicy::new(account_id, committed_floor)])
        );
    }

    #[test]
    fn production_state_storage_does_not_use_rusqlite() {
        let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sqlite.rs");
        let source = fs::read_to_string(&source_path)
            .unwrap_or_else(|error| panic!("state sqlite source should be readable: {error}"));
        let production_source = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(source.as_str());
        let lines = production_source.lines().collect::<Vec<_>>();
        let forbidden_lines = lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                let references_rusqlite = line.contains("rusqlite::")
                    || line.contains("use rusqlite")
                    || line.contains("&rusqlite");
                let fenced_by_fixture_feature =
                    index > 0 && lines[index - 1].contains("sync-rusqlite-fixtures");
                (references_rusqlite && !fenced_by_fixture_feature).then_some(format!(
                    "{}:{}",
                    index + 1,
                    line.trim()
                ))
            })
            .collect::<Vec<_>>();

        assert!(
            forbidden_lines.is_empty(),
            "production state storage must be SQLx-only; forbidden rusqlite lines: {forbidden_lines:?}"
        );
    }

    #[test]
    fn sqlite_migration_roundtrips_account_and_quota_snapshot() {
        let temp_dir = TestTempDir::new("migration_roundtrip");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };

        assert_eq!(store.schema_version(), 13);

        let account_id = match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let account = AccountRecord::new(account_id.clone(), "primary", AccountStatus::Enabled);
        if let Err(error) = store.upsert_account(&account) {
            panic!("account should persist: {error}");
        }

        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(100)
                .with_route_band("responses", 42)
                .with_reset_unix_seconds(160)
                .with_reset_credits_available(1)
                .with_stale_penalty(false);
        if let Err(error) = store.upsert_quota_snapshot(&snapshot) {
            panic!("quota snapshot should persist: {error}");
        }

        assert_eq!(store.load_account(&account_id), Ok(Some(account)));
        assert_eq!(store.load_quota_snapshot(&account_id), Ok(Some(snapshot)));
    }

    #[test]
    fn quota_snapshots_are_partitioned_by_route_band_for_one_account() {
        let temp_dir = TestTempDir::new("quota_route_band_partition");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_route_partition");
        let responses_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_000)
                .with_route_band("responses", 90);
        let models_snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_010)
                .with_route_band("models", 7);

        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &responses_snapshot) {
            panic!("responses quota snapshot should persist: {error}");
        }
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &models_snapshot) {
            panic!("models quota snapshot should persist: {error}");
        }

        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "responses"),
            Ok(Some(responses_snapshot))
        );
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(&store, &account_id, "models"),
            Ok(Some(models_snapshot))
        );
    }

    #[test]
    fn selector_input_reads_durable_per_window_rows_without_status_renderer() {
        let temp_dir = TestTempDir::new("selector_input_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_selector_windows");
        let account = AccountRecord::new(account_id.clone(), "selector", AccountStatus::Enabled)
            .with_active_credential_generation(3);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Stale,
        )
        .with_remaining_headroom(72)
        .with_reset_unix_seconds(19_000)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Stale,
        )
        .with_remaining_headroom(41)
        .with_reset_unix_seconds(700_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &short_window) {
            panic!("short window should persist: {error}");
        }
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &weekly_window)
        {
            panic!("weekly window should persist: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };

        assert_eq!(selector_inputs.len(), 1);
        let input = &selector_inputs[0];
        assert_eq!(input.account_id(), &account_id);
        assert_eq!(input.account_label(), "selector");
        assert_eq!(input.account_status(), AccountStatus::Enabled);
        assert_eq!(input.active_credential_generation(), Some(3));
        assert_eq!(input.route_band(), "responses");
        assert_eq!(input.windows(), &[weekly_window, short_window]);
        let effective = input
            .windows()
            .iter()
            .find(|window| window.effective())
            .unwrap_or_else(|| panic!("effective selector window should exist"));
        assert_eq!(effective.limit_window_seconds(), 604_800);
        assert_eq!(effective.status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(effective.remaining_headroom(), 41);
        assert_eq!(effective.reset_unix_seconds(), Some(700_000));
    }

    #[tokio::test]
    async fn async_selector_input_matches_sync_repository_projection() {
        let temp_dir = TestTempDir::new("async_selector_input_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_async_selector_windows");
        let account =
            AccountRecord::new(account_id.clone(), "async-selector", AccountStatus::Enabled)
                .with_active_credential_generation(9);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("account should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(88)
        .with_reset_unix_seconds(20_000)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(55)
        .with_reset_unix_seconds(700_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&sync_store, &short_window)
        {
            panic!("short window should persist: {error}");
        }
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&sync_store, &weekly_window)
        {
            panic!("weekly window should persist: {error}");
        }

        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let sync_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &sync_store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("sync selector input should load: {error}"),
        };
        let async_inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
            &async_store,
            "responses",
            1_000,
        )
        .await
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("async selector input should load: {error}"),
        };

        assert_eq!(async_inputs, sync_inputs);
    }

    #[tokio::test]
    async fn async_read_only_store_reads_without_allowing_writes() {
        let temp_dir = TestTempDir::new("async_read_only_store");
        let database_path = temp_dir.path().join("state.sqlite");
        let writable_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("writable async state store should open and migrate: {error}"),
        };
        let account = AccountRecord::new(
            account_id("acct_async_read_only"),
            "read-only",
            AccountStatus::Enabled,
        );
        if let Err(error) = writable_store.upsert_account(&account).await {
            panic!("writable async store should persist account: {error}");
        }
        if let Err(error) = writable_store.close().await {
            panic!("writable async store should close cleanly: {error}");
        }
        drop(writable_store);

        let read_only_store = match AsyncSqliteStateStore::open_read_only(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("read-only async state store should open: {error}"),
        };
        let accounts = match read_only_store.list_accounts().await {
            Ok(accounts) => accounts,
            Err(error) => panic!("read-only async store should list accounts: {error}"),
        };
        assert_eq!(accounts, vec![account]);

        let write_error = read_only_store
            .upsert_account(&AccountRecord::new(
                account_id("acct_async_read_only_blocked"),
                "blocked",
                AccountStatus::Enabled,
            ))
            .await;
        assert!(
            write_error.is_err(),
            "read-only async store must reject writes"
        );
    }

    #[tokio::test]
    async fn async_read_only_store_reports_missing_current_schema_without_writes() {
        let temp_dir = TestTempDir::new("async_read_only_missing_schema");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v10_database_missing_async_projection_tables(&database_path);
        let raw = Connection::open(&database_path).expect("fixture should reopen");
        raw.pragma_update(None, "user_version", 13_i64)
            .expect("fixture should claim current schema");
        drop(raw);

        let error = expect_error(
            AsyncSqliteStateStore::open_read_only(&database_path).await,
            "read-only open should fail before status queries hit missing tables",
        );

        assert_eq!(
            error,
            StateStoreError::MissingReadOnlySchemaObject {
                object_kind: "table",
                object_name: "quota_history_observations",
            }
        );
    }

    #[tokio::test]
    async fn sync_seeded_state_store_is_accepted_by_quota_status_read_only_schema() {
        let temp_dir = TestTempDir::new("sync_seeded_read_only_quota_status");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_sync_seeded_quota_status");
        let account = AccountRecord::new(account_id.clone(), "sync-seeded", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("sync seed should persist account: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(91)
        .with_reset_unix_seconds(18_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(54)
        .with_reset_unix_seconds(604_800)
        .with_observed_unix_seconds(1_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&sync_store, &short_window)
        {
            panic!("sync seed should persist short selector window: {error}");
        }
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&sync_store, &weekly_window)
        {
            panic!("sync seed should persist weekly selector window: {error}");
        }
        drop(sync_store);

        let read_only_store = match AsyncSqliteStateStore::open_read_only(&database_path).await {
            Ok(store) => store,
            Err(error) => {
                panic!(
                    "sync-seeded state database should satisfy read-only quota status schema: {error}"
                )
            }
        };
        let observations = match AsyncQuotaHistoryRepository::quota_history_observations_for_window(
            &read_only_store,
            &account_id,
            "responses",
            604_800,
            0,
            2_000,
        )
        .await
        {
            Ok(observations) => observations,
            Err(error) => {
                panic!(
                    "quota status read-only quota history query should find current schema: {error}"
                )
            }
        };

        assert!(
            observations.is_empty(),
            "sync-seeded state should start with no quota history observations"
        );
    }

    #[tokio::test]
    async fn async_writable_store_enables_wal_journal_mode() {
        let temp_dir = TestTempDir::new("async_wal_journal_mode");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        if let Err(error) = store.close().await {
            panic!("async state store should close cleanly: {error}");
        }
        drop(store);

        let connection = match Connection::open(&database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("sqlite database should open for journal inspection: {error}"),
        };
        let journal_mode: String =
            match connection.query_row("PRAGMA journal_mode", [], |row| row.get(0)) {
                Ok(journal_mode) => journal_mode,
                Err(error) => panic!("journal mode should be readable: {error}"),
            };
        assert_eq!(journal_mode.to_ascii_lowercase(), "wal");
    }

    #[tokio::test]
    async fn async_quota_history_appends_queries_and_purges_old_observations() {
        let temp_dir = TestTempDir::new("async_quota_history");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_history");
        let old_observation = quota_history_observation(
            account_id.clone(),
            "responses",
            18_000,
            100,
            91,
            Some(18_100),
        );
        let first_observation = quota_history_observation(
            account_id.clone(),
            "responses",
            18_000,
            10_000,
            88,
            Some(28_000),
        )
        .with_effective(true)
        .with_reset_credits_available(1);
        let second_observation = quota_history_observation(
            account_id.clone(),
            "responses",
            18_000,
            10_900,
            76,
            Some(28_000),
        )
        .with_refresh_outcome(QuotaHistoryRefreshOutcome::Failure {
            error_class: QuotaRefreshErrorClass::RateLimited,
        });
        let other_window = quota_history_observation(
            account_id.clone(),
            "responses",
            604_800,
            10_900,
            50,
            Some(615_700),
        );

        for observation in [
            old_observation,
            first_observation.clone(),
            second_observation.clone(),
            other_window,
        ] {
            if let Err(error) =
                AsyncQuotaHistoryRepository::append_quota_history_observation(&store, &observation)
                    .await
            {
                panic!("quota history observation should append: {error}");
            }
        }
        if let Err(error) =
            AsyncQuotaHistoryRepository::purge_quota_history_before(&store, 1_000).await
        {
            panic!("old quota history should purge: {error}");
        }

        let observations = match AsyncQuotaHistoryRepository::quota_history_observations_for_window(
            &store,
            &account_id,
            "responses",
            18_000,
            1_000,
            11_000,
        )
        .await
        {
            Ok(observations) => observations,
            Err(error) => panic!("quota history observations should load: {error}"),
        };

        assert_eq!(observations, vec![first_observation, second_observation]);
    }

    #[tokio::test]
    async fn sqlx_active_client_leases_count_release_and_prune() {
        let temp_dir = TestTempDir::new("async_active_client_leases");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let alpha = account_id("acct_active_alpha");
        let beta = account_id("acct_active_beta");

        if let Err(error) = store
            .record_active_client_acquired(
                "responses",
                "process-a",
                &ReservationId::new("reservation_alpha_1"),
                &alpha,
                1_000,
                2,
            )
            .await
        {
            panic!("alpha first lease should persist: {error}");
        }
        if let Err(error) = store
            .record_active_client_acquired(
                "responses",
                "process-a",
                &ReservationId::new("reservation_alpha_2"),
                &alpha,
                1_005,
                8,
            )
            .await
        {
            panic!("alpha second lease should persist: {error}");
        }
        if let Err(error) = store
            .record_active_client_acquired(
                "responses",
                "process-a",
                &ReservationId::new("reservation_beta_1"),
                &beta,
                1_006,
                8,
            )
            .await
        {
            panic!("beta lease should persist: {error}");
        }

        let counts = match store
            .active_client_counts_for_route_band("responses", 1_010, 100)
            .await
        {
            Ok(counts) => counts,
            Err(error) => panic!("active client counts should load: {error}"),
        };
        assert_eq!(
            counts,
            vec![
                crate::sqlite::ActiveClientCount::new(alpha.clone(), 2, 10),
                crate::sqlite::ActiveClientCount::new(beta.clone(), 1, 8),
            ]
        );

        if let Err(error) = store
            .record_active_client_released(
                "responses",
                "process-a",
                &ReservationId::new("reservation_alpha_1"),
                1_010,
            )
            .await
        {
            panic!("alpha lease should release: {error}");
        }
        let counts = match store
            .active_client_counts_for_route_band("responses", 1_010, 100)
            .await
        {
            Ok(counts) => counts,
            Err(error) => panic!("active client counts should reload: {error}"),
        };
        assert_eq!(
            counts,
            vec![
                crate::sqlite::ActiveClientCount::new(alpha, 1, 8),
                crate::sqlite::ActiveClientCount::new(beta, 1, 8),
            ]
        );

        let counts = match store
            .active_client_counts_for_route_band("responses", 1_200, 100)
            .await
        {
            Ok(counts) => counts,
            Err(error) => panic!("stale active client counts should prune: {error}"),
        };
        assert!(counts.is_empty());
    }

    #[tokio::test]
    async fn sqlx_active_client_leases_do_not_collide_across_process_runs() {
        let temp_dir = TestTempDir::new("async_active_client_process_collision");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_active_shared");
        let reservation_id = ReservationId::new("reservation_1");

        if let Err(error) = store
            .record_active_client_acquired(
                "responses",
                "process-a",
                &reservation_id,
                &account,
                1_000,
                2,
            )
            .await
        {
            panic!("process-a lease should persist: {error}");
        }
        if let Err(error) = store
            .record_active_client_acquired(
                "responses",
                "process-b",
                &reservation_id,
                &account,
                1_001,
                8,
            )
            .await
        {
            panic!("process-b lease should persist without overwriting process-a: {error}");
        }

        let counts = match store
            .active_client_counts_for_route_band("responses", 1_010, 100)
            .await
        {
            Ok(counts) => counts,
            Err(error) => panic!("active client counts should load: {error}"),
        };
        assert_eq!(
            counts,
            vec![crate::sqlite::ActiveClientCount::new(
                account.clone(),
                2,
                10
            )]
        );

        if let Err(error) = store
            .record_active_client_released("responses", "process-a", &reservation_id, 1_010)
            .await
        {
            panic!("process-a release should not delete process-b lease: {error}");
        }
        let counts = match store
            .active_client_counts_for_route_band("responses", 1_010, 100)
            .await
        {
            Ok(counts) => counts,
            Err(error) => panic!("active client counts should reload: {error}"),
        };
        assert_eq!(
            counts,
            vec![crate::sqlite::ActiveClientCount::new(account, 1, 8)]
        );
    }

    #[tokio::test]
    async fn sqlx_active_session_events_retain_completed_sessions_after_release() {
        let temp_dir = TestTempDir::new("async_active_session_events");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_completed_session");
        let reservation_id = ReservationId::new("reservation_completed_1");

        store
            .record_active_client_acquired(
                "responses",
                "process-a",
                &reservation_id,
                &account,
                1_000,
                8,
            )
            .await
            .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
        store
            .record_active_client_released("responses", "process-a", &reservation_id, 1_100)
            .await
            .unwrap_or_else(|error| panic!("session release should persist: {error}"));

        let counts = store
            .active_client_counts_for_route_band("responses", 1_100, 300)
            .await
            .unwrap_or_else(|error| panic!("active counts should load: {error}"));
        assert!(
            counts.is_empty(),
            "released sessions should not remain active"
        );

        let events = store
            .active_session_events_for_route_band("responses")
            .await
            .unwrap_or_else(|error| panic!("active session events should load: {error}"));
        assert_eq!(
            events,
            vec![
                crate::sqlite::ActiveSessionEvent::new(
                    account.clone(),
                    "responses",
                    "process-a",
                    reservation_id.clone(),
                    crate::sqlite::ActiveSessionEventKind::Acquired,
                    1_000,
                ),
                crate::sqlite::ActiveSessionEvent::new(
                    account,
                    "responses",
                    "process-a",
                    reservation_id,
                    crate::sqlite::ActiveSessionEventKind::Released,
                    1_100,
                )
                .with_session_interval(1_000, Some(1_100), "unknown"),
            ]
        );
    }

    #[tokio::test]
    async fn sqlx_active_session_release_timestamp_bounds_rollup_interval() {
        let temp_dir = TestTempDir::new("async_active_session_release_timestamp");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_release_timestamp");
        let reservation_id = ReservationId::new("reservation_release_timestamp");

        store
            .record_active_client_acquired(
                "responses",
                "process-release",
                &reservation_id,
                &account,
                1_000,
                8,
            )
            .await
            .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
        store
            .record_active_client_released("responses", "process-release", &reservation_id, 1_100)
            .await
            .unwrap_or_else(|error| panic!("session release should persist: {error}"));
        store
            .refresh_active_session_rollups_for_interval("responses", 1_000, 1_300, 300)
            .await
            .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

        let rollups = store
            .active_session_rollups_for_route_band("responses", 1_000, 1_300)
            .await
            .unwrap_or_else(|error| panic!("rollups should load: {error}"));
        let active_session_seconds = rollups
            .iter()
            .map(crate::sqlite::ActiveSessionRollup::active_session_seconds)
            .sum::<u64>();
        assert_eq!(active_session_seconds, 100);
    }

    #[tokio::test]
    async fn sqlx_active_session_rollups_clip_partial_buckets_and_overlap() {
        let temp_dir = TestTempDir::new("async_active_session_rollups");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_rollup_session");
        let first = ReservationId::new("reservation_rollup_1");
        let second = ReservationId::new("reservation_rollup_2");

        store
            .record_active_client_acquired("responses", "process-a", &first, &account, 100, 8)
            .await
            .unwrap_or_else(|error| panic!("first acquire should persist: {error}"));
        store
            .record_active_client_acquired("responses", "process-a", &second, &account, 160, 8)
            .await
            .unwrap_or_else(|error| panic!("second acquire should persist: {error}"));
        store
            .record_active_client_released("responses", "process-a", &first, 220)
            .await
            .unwrap_or_else(|error| panic!("first release should persist: {error}"));
        store
            .record_active_client_released("responses", "process-a", &second, 260)
            .await
            .unwrap_or_else(|error| panic!("second release should persist: {error}"));

        store
            .refresh_active_session_rollups_for_interval("responses", 120, 240, 300)
            .await
            .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));
        let rollups = store
            .active_session_rollups_for_route_band("responses", 120, 240)
            .await
            .unwrap_or_else(|error| panic!("rollups should load: {error}"));

        assert_eq!(
            rollups,
            vec![
                crate::sqlite::ActiveSessionRollup::new(account, "responses", 0, 300, 180, 2)
                    .with_terminal_counts(1, 0)
            ]
        );
    }

    #[tokio::test]
    async fn sqlx_active_session_history_schema_matches_spec_terminal_counters() {
        let temp_dir = TestTempDir::new("async_active_session_schema_spec");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!("sqlite://{}", database_path.display()))
            .await
            .unwrap_or_else(|error| panic!("schema check pool should open: {error}"));

        let event_columns = sqlx::query("PRAGMA table_info(active_session_events)")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("active_session_events schema should load: {error}"))
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        for required_column in [
            "logical_session_id",
            "session_started_unix_seconds",
            "session_ended_unix_seconds",
            "transport_kind",
        ] {
            assert!(
                event_columns.iter().any(|column| column == required_column),
                "active_session_events must include {required_column}: {event_columns:?}"
            );
        }

        let rollup_columns = sqlx::query("PRAGMA table_info(active_session_rollups)")
            .fetch_all(&pool)
            .await
            .unwrap_or_else(|error| panic!("active_session_rollups schema should load: {error}"))
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        for required_column in ["completed_sessions", "stale_purged_sessions"] {
            assert!(
                rollup_columns
                    .iter()
                    .any(|column| column == required_column),
                "active_session_rollups must include {required_column}: {rollup_columns:?}"
            );
        }

        let account = account_id("acct_schema_spec");
        let released = ReservationId::new("reservation_schema_released");
        let stale = ReservationId::new("reservation_schema_stale");
        store
            .record_active_client_acquired("responses", "process-a", &released, &account, 10, 8)
            .await
            .unwrap_or_else(|error| panic!("released acquire should persist: {error}"));
        store
            .record_active_client_released("responses", "process-a", &released, 40)
            .await
            .unwrap_or_else(|error| panic!("released terminal event should persist: {error}"));
        store
            .record_active_client_acquired("responses", "process-a", &stale, &account, 50, 8)
            .await
            .unwrap_or_else(|error| panic!("stale acquire should persist: {error}"));
        store
            .active_client_counts_for_route_band("responses", 500, 300)
            .await
            .unwrap_or_else(|error| panic!("stale prune should persist terminal event: {error}"));
        store
            .refresh_active_session_rollups_for_interval("responses", 0, 600, 300)
            .await
            .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

        let rollups = store
            .active_session_rollups_for_route_band("responses", 0, 600)
            .await
            .unwrap_or_else(|error| panic!("rollups should load: {error}"));
        assert_eq!(
            rollups
                .iter()
                .map(crate::sqlite::ActiveSessionRollup::completed_sessions)
                .sum::<u32>(),
            1
        );
        assert_eq!(
            rollups
                .iter()
                .map(crate::sqlite::ActiveSessionRollup::stale_purged_sessions)
                .sum::<u32>(),
            1
        );
    }

    #[tokio::test]
    async fn sqlx_active_session_stale_prune_records_terminal_event_and_rollup() {
        let temp_dir = TestTempDir::new("async_active_session_stale_prune");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_stale_session");
        let reservation_id = ReservationId::new("reservation_stale_1");

        store
            .record_active_client_acquired(
                "responses",
                "process-stale",
                &reservation_id,
                &account,
                100,
                8,
            )
            .await
            .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));

        let counts = store
            .active_client_counts_for_route_band("responses", 1_000, 300)
            .await
            .unwrap_or_else(|error| panic!("active counts should prune stale leases: {error}"));
        assert!(counts.is_empty(), "stale lease should no longer be active");

        let events = store
            .active_session_events_for_route_band("responses")
            .await
            .unwrap_or_else(|error| panic!("active session events should load: {error}"));
        assert_eq!(
            events,
            vec![
                crate::sqlite::ActiveSessionEvent::new(
                    account.clone(),
                    "responses",
                    "process-stale",
                    reservation_id.clone(),
                    crate::sqlite::ActiveSessionEventKind::Acquired,
                    100,
                ),
                crate::sqlite::ActiveSessionEvent::new(
                    account.clone(),
                    "responses",
                    "process-stale",
                    reservation_id,
                    crate::sqlite::ActiveSessionEventKind::StalePurged,
                    1_000,
                )
                .with_session_interval(100, Some(1_000), "unknown"),
            ]
        );

        store
            .refresh_active_session_rollups_for_interval("responses", 0, 1_200, 300)
            .await
            .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));
        let rollups = store
            .active_session_rollups_for_route_band("responses", 0, 1_200)
            .await
            .unwrap_or_else(|error| panic!("rollups should load: {error}"));
        assert_eq!(
            rollups,
            vec![
                crate::sqlite::ActiveSessionRollup::new(
                    account.clone(),
                    "responses",
                    0,
                    300,
                    200,
                    1,
                ),
                crate::sqlite::ActiveSessionRollup::new(
                    account.clone(),
                    "responses",
                    300,
                    600,
                    300,
                    1,
                ),
                crate::sqlite::ActiveSessionRollup::new(
                    account.clone(),
                    "responses",
                    600,
                    900,
                    300,
                    1,
                ),
                crate::sqlite::ActiveSessionRollup::new(account, "responses", 900, 1_200, 100, 1)
                    .with_terminal_counts(0, 1),
            ]
        );
    }

    #[tokio::test]
    async fn writable_selection_projection_terminalizes_stale_active_leases() {
        let temp_dir = TestTempDir::new("selection_projection_stale_lease_cleanup");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_stale_cleanup");
        let reservation_id = ReservationId::new("reservation_projection_stale_cleanup");
        let account_record =
            AccountRecord::new(account.clone(), "projection-stale", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(90)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(100),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        store
            .record_active_client_acquired(
                "responses",
                "process-projection-stale",
                &reservation_id,
                &account,
                100,
                8,
            )
            .await
            .unwrap_or_else(|error| panic!("stale lease should acquire: {error}"));

        let projection = project_route_band_selection_inputs(&store, "responses", 1_000, 300)
            .await
            .unwrap_or_else(|error| panic!("writable projection should load: {error}"));

        assert_eq!(projection.accounts().len(), 1);
        assert_eq!(projection.accounts()[0].current_active_sessions(), 0);
        let events = store
            .active_session_events_for_route_band("responses")
            .await
            .unwrap_or_else(|error| panic!("active session events should load: {error}"));
        assert!(
            events.iter().any(|event| {
                event.event_kind() == crate::sqlite::ActiveSessionEventKind::StalePurged
                    && event.reservation_id() == &reservation_id
            }),
            "writable projection should terminalize stale persisted leases"
        );
    }

    #[tokio::test]
    async fn sqlx_v8_migration_preserves_current_leases_without_synthetic_session_history() {
        let temp_dir = TestTempDir::new("async_v8_active_session_migration");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v8_database_with_current_active_lease(&database_path);

        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async v8 state store should migrate to v9: {error}"),
        };

        assert_eq!(
            store
                .schema_version()
                .await
                .unwrap_or_else(|error| panic!("schema version should load: {error}")),
            13
        );
        assert_eq!(
            store
                .active_client_counts_for_route_band("responses", 150, 300)
                .await
                .unwrap_or_else(|error| panic!("active lease should survive migration: {error}")),
            vec![crate::sqlite::ActiveClientCount::new(
                account_id("acct_v8_active_lease"),
                1,
                8,
            )]
        );
        assert!(
            store
                .active_session_events_for_route_band("responses")
                .await
                .unwrap_or_else(|error| panic!("session history should load: {error}"))
                .is_empty(),
            "v9 migration must not synthesize completed session history from current leases"
        );
    }

    #[tokio::test]
    async fn sqlx_current_schema_repairs_partial_v10_active_session_columns() {
        let temp_dir = TestTempDir::new("async_partial_v10_active_session_repair");
        let database_path = temp_dir.path().join("state.sqlite");
        create_partial_v10_database_missing_active_session_columns(&database_path);

        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("partial v10 state store should open and repair: {error}"),
        };
        let account = account_id("acct_partial_v10");
        let reservation = ReservationId::new("reservation-partial-v10");

        store
            .record_active_client_acquired(
                "models",
                "process-partial",
                &reservation,
                &account,
                100,
                1,
            )
            .await
            .unwrap_or_else(|error| {
                panic!("active client mirror should write after partial v10 repair: {error}")
            });

        assert_eq!(
            store
                .active_client_counts_for_route_band("models", 120, 300)
                .await
                .unwrap_or_else(|error| panic!("active client counts should load: {error}")),
            vec![crate::sqlite::ActiveClientCount::new(account, 1, 1)]
        );
    }

    #[tokio::test]
    async fn sqlx_active_session_rollup_retention_purges_old_buckets() {
        let temp_dir = TestTempDir::new("async_active_session_rollup_retention");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_rollup_retention");
        let reservation_id = ReservationId::new("reservation_retention_1");

        store
            .record_active_client_acquired(
                "responses",
                "process-retention",
                &reservation_id,
                &account,
                0,
                8,
            )
            .await
            .unwrap_or_else(|error| panic!("session acquire should persist: {error}"));
        store
            .record_active_client_released("responses", "process-retention", &reservation_id, 700)
            .await
            .unwrap_or_else(|error| panic!("session release should persist: {error}"));
        store
            .refresh_active_session_rollups_for_interval("responses", 0, 900, 300)
            .await
            .unwrap_or_else(|error| panic!("rollups should refresh: {error}"));

        store
            .purge_active_session_rollups_before(600)
            .await
            .unwrap_or_else(|error| panic!("old rollups should purge: {error}"));
        let rollups = store
            .active_session_rollups_for_route_band("responses", 0, 900)
            .await
            .unwrap_or_else(|error| panic!("rollups should load after purge: {error}"));
        assert_eq!(
            rollups,
            vec![
                crate::sqlite::ActiveSessionRollup::new(
                    account.clone(),
                    "responses",
                    300,
                    600,
                    300,
                    1,
                ),
                crate::sqlite::ActiveSessionRollup::new(account, "responses", 600, 900, 100, 1)
                    .with_terminal_counts(1, 0),
            ]
        );
    }

    #[tokio::test]
    async fn selection_projection_refreshes_session_rollups_for_candidate_burn() {
        let temp_dir = TestTempDir::new("selection_projection_session_count");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_session_count");
        let account_record =
            AccountRecord::new(account.clone(), "projection", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(45)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(3_700),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
            store
                .append_quota_history_observation(&quota_history_observation(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    observed_unix_seconds,
                    remaining_headroom,
                    Some(100_000),
                ))
                .await
                .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
        }
        let historical_session = ReservationId::new("reservation_projection_history");
        store
            .record_active_client_acquired(
                "responses",
                "process-projection-history",
                &historical_session,
                &account,
                100,
                99,
            )
            .await
            .unwrap_or_else(|error| panic!("historical session should acquire: {error}"));
        store
            .record_active_client_released(
                "responses",
                "process-projection-history",
                &historical_session,
                3_700,
            )
            .await
            .unwrap_or_else(|error| panic!("historical session should release: {error}"));
        for index in 0..2 {
            let current_session =
                ReservationId::new(format!("reservation_projection_current_{index}"));
            store
                .record_active_client_acquired(
                    "responses",
                    "process-projection-current",
                    &current_session,
                    &account,
                    3_800 + index,
                    99,
                )
                .await
                .unwrap_or_else(|error| panic!("current session should acquire: {error}"));
        }

        let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
            .await
            .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

        assert_eq!(projection.accounts().len(), 1);
        let projected_account = &projection.accounts()[0];
        assert_eq!(projected_account.current_active_sessions(), 2);
        let weekly_window = projected_account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
            .unwrap_or_else(|| panic!("weekly selector window should project"));
        assert_eq!(
            weekly_window.per_connection_burn_basis_points_per_hour(),
            Some(500)
        );
        assert_eq!(
            weekly_window.burn_rate_confidence(),
            QuotaRunRateConfidence::Normal
        );
    }

    #[tokio::test]
    async fn projection_keeps_per_connection_burn_invariant_across_active_overrides() {
        let temp_dir = TestTempDir::new("selection_projection_per_connection_invariant");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_per_connection");
        let account_record =
            AccountRecord::new(account.clone(), "projection", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(45)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(3_700),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
            store
                .append_quota_history_observation(&quota_history_observation(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    observed_unix_seconds,
                    remaining_headroom,
                    Some(100_000),
                ))
                .await
                .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
        }
        let historical_session = ReservationId::new("reservation_projection_invariant_history");
        store
            .record_active_client_acquired(
                "responses",
                "process-projection-invariant-history",
                &historical_session,
                &account,
                100,
                99,
            )
            .await
            .unwrap_or_else(|error| panic!("historical session should acquire: {error}"));
        store
            .record_active_client_released(
                "responses",
                "process-projection-invariant-history",
                &historical_session,
                3_700,
            )
            .await
            .unwrap_or_else(|error| panic!("historical session should release: {error}"));

        let projection_for_active_override = |active_sessions: u32| {
            let store = &store;
            let account = account.clone();
            async move {
                let mut overrides = HashMap::new();
                overrides.insert(account.clone(), active_sessions);
                let projection = project_route_band_selection_inputs_with_active_counts(
                    store,
                    "responses",
                    3_900,
                    7_200,
                    Some(&overrides),
                )
                .await
                .unwrap_or_else(|error| panic!("selection projection should load: {error}"));
                let projected_account = projection
                    .accounts()
                    .first()
                    .unwrap_or_else(|| panic!("projected account should exist"));
                let weekly_window = projected_account
                    .windows()
                    .iter()
                    .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
                    .unwrap_or_else(|| panic!("weekly selector window should project"));
                (
                    weekly_window.per_connection_burn_basis_points_per_hour(),
                    weekly_window.projected_exhaustion_unix_seconds(),
                )
            }
        };

        let (zero_active_burn, zero_active_exhaustion) = projection_for_active_override(0).await;
        let (one_active_burn, one_active_exhaustion) = projection_for_active_override(1).await;
        let (two_active_burn, two_active_exhaustion) = projection_for_active_override(2).await;

        assert_eq!(zero_active_burn, Some(500));
        assert_eq!(
            one_active_burn, zero_active_burn,
            "per-connection burn must not change when current active sessions change"
        );
        assert_eq!(
            two_active_burn, zero_active_burn,
            "per-connection burn must not be pre-multiplied by projected active sessions"
        );
        assert_eq!(zero_active_exhaustion, Some(36_300));
        assert_eq!(
            one_active_exhaustion,
            Some(20_100),
            "projected exhaustion must use the candidate aggregate burn for one current active session plus the new session"
        );
        assert_eq!(
            two_active_exhaustion,
            Some(14_700),
            "projected exhaustion must continue shrinking as active-session overrides grow"
        );
    }

    #[tokio::test]
    async fn selection_projection_downgrades_zero_active_session_history() {
        let temp_dir = TestTempDir::new("selection_projection_zero_active_history");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_zero_active");
        let account_record =
            AccountRecord::new(account.clone(), "zero-active", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(45)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(3_700),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
            store
                .append_quota_history_observation(&quota_history_observation(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    observed_unix_seconds,
                    remaining_headroom,
                    Some(100_000),
                ))
                .await
                .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
        }

        let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
            .await
            .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

        assert_eq!(projection.accounts().len(), 1);
        let projected_account = &projection.accounts()[0];
        let weekly_window = projected_account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
            .unwrap_or_else(|| panic!("weekly selector window should project"));
        assert_eq!(
            weekly_window.per_connection_burn_basis_points_per_hour(),
            None
        );
        assert_eq!(
            weekly_window.aggregate_burn_basis_points_per_hour(),
            Some(500)
        );
        assert_eq!(
            weekly_window.burn_rate_confidence(),
            QuotaRunRateConfidence::Low,
            "quota burn with zero active-session history must not keep normal confidence"
        );
    }

    #[tokio::test]
    async fn projection_never_manufactures_zero_from_one_observation() {
        let temp_dir = TestTempDir::new("selection_projection_one_observation_no_fake_zero");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_one_observation");
        let account_record =
            AccountRecord::new(account.clone(), "one-observation", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(45)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(3_700),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        store
            .append_quota_history_observation(&quota_history_observation(
                account.clone(),
                "responses",
                V1_WEEKLY_WINDOW_SECONDS,
                3_700,
                45,
                Some(100_000),
            ))
            .await
            .unwrap_or_else(|error| panic!("quota history should persist: {error}"));

        let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
            .await
            .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

        let projected_account = projection
            .accounts()
            .first()
            .unwrap_or_else(|| panic!("projected account should exist"));
        let weekly_window = projected_account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
            .unwrap_or_else(|| panic!("weekly selector window should project"));
        assert_eq!(
            weekly_window.per_connection_burn_basis_points_per_hour(),
            None,
            "one quota observation is insufficient and must not become fake zero burn"
        );
        assert_eq!(
            weekly_window.burn_rate_confidence(),
            QuotaRunRateConfidence::Insufficient
        );
    }

    #[tokio::test]
    async fn selection_projection_downgrades_partial_active_session_history() {
        let temp_dir = TestTempDir::new("selection_projection_partial_active_history");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let account = account_id("acct_projection_partial_active");
        let account_record =
            AccountRecord::new(account.clone(), "partial-active", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        store
            .upsert_account(&account_record)
            .await
            .unwrap_or_else(|error| panic!("account should persist: {error}"));
        store
            .upsert_selector_quota_window(
                &PersistedSelectorQuotaWindow::new(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    SelectorQuotaWindowStatus::Eligible,
                )
                .with_remaining_headroom(45)
                .with_reset_unix_seconds(100_000)
                .with_effective(true)
                .with_observed_unix_seconds(3_700),
            )
            .await
            .unwrap_or_else(|error| panic!("selector window should persist: {error}"));
        for (observed_unix_seconds, remaining_headroom) in [(100, 50), (1_900, 48), (3_700, 45)] {
            store
                .append_quota_history_observation(&quota_history_observation(
                    account.clone(),
                    "responses",
                    V1_WEEKLY_WINDOW_SECONDS,
                    observed_unix_seconds,
                    remaining_headroom,
                    Some(100_000),
                ))
                .await
                .unwrap_or_else(|error| panic!("quota history should persist: {error}"));
        }
        let late_session = ReservationId::new("reservation_projection_late_history");
        store
            .record_active_client_acquired(
                "responses",
                "process-projection-late-history",
                &late_session,
                &account,
                3_000,
                99,
            )
            .await
            .unwrap_or_else(|error| panic!("late session should acquire: {error}"));
        store
            .record_active_client_released(
                "responses",
                "process-projection-late-history",
                &late_session,
                3_700,
            )
            .await
            .unwrap_or_else(|error| panic!("late session should release: {error}"));

        let projection = project_route_band_selection_inputs(&store, "responses", 3_900, 7_200)
            .await
            .unwrap_or_else(|error| panic!("selection projection should load: {error}"));

        assert_eq!(projection.accounts().len(), 1);
        let projected_account = &projection.accounts()[0];
        let weekly_window = projected_account
            .windows()
            .iter()
            .find(|window| window.window_seconds() == V1_WEEKLY_WINDOW_SECONDS)
            .unwrap_or_else(|| panic!("weekly selector window should project"));
        assert_eq!(
            weekly_window.per_connection_burn_basis_points_per_hour(),
            None,
            "partial active-session history must fall back to aggregate quota burn"
        );
        assert_eq!(
            weekly_window.aggregate_burn_basis_points_per_hour(),
            Some(500),
            "partial active-session history keeps aggregate fallback separate"
        );
        assert_eq!(
            weekly_window.burn_rate_confidence(),
            QuotaRunRateConfidence::Low,
            "partial active-session history must not keep normal confidence"
        );
    }

    #[tokio::test]
    async fn async_quota_exhaustion_marks_route_band_windows_ineligible() {
        let temp_dir = TestTempDir::new("async_quota_exhaustion");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_quota_exhausted");
        let account = AccountRecord::new(account_id.clone(), "exhausted", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("account should persist: {error}");
        }
        for (limit_window_seconds, remaining_headroom, effective) in
            [(18_000, 70, true), (604_800, 55, false)]
        {
            let window = PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                limit_window_seconds,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(remaining_headroom)
            .with_effective(effective)
            .with_observed_unix_seconds(1_000)
            .with_reset_unix_seconds(10_000 + limit_window_seconds);
            if let Err(error) =
                SelectorQuotaRepository::upsert_selector_window(&sync_store, &window)
            {
                panic!("selector window should persist: {error}");
            }
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };

        if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
            &async_store,
            &account_id,
            "responses",
            2_000,
        )
        .await
        {
            panic!("quota exhaustion should persist: {error}");
        }

        let inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
            &async_store,
            "responses",
            2_000,
        )
        .await
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        let input = inputs
            .iter()
            .find(|input| input.account_id() == &account_id)
            .unwrap_or_else(|| panic!("exhausted account selector input should exist"));
        assert_eq!(input.windows().len(), 2);
        assert!(
            input
                .windows()
                .iter()
                .all(|window| window.status() == SelectorQuotaWindowStatus::Ineligible)
        );
        assert!(
            input
                .windows()
                .iter()
                .all(|window| window.remaining_headroom() == 0)
        );
        assert!(
            input
                .windows()
                .iter()
                .all(|window| window.observed_unix_seconds() == 2_000)
        );
    }

    #[tokio::test]
    async fn async_quota_exhaustion_without_existing_windows_blocks_expected_windows() {
        let temp_dir = TestTempDir::new("async_quota_exhaustion_no_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_unknown_exhausted");
        let account = AccountRecord::new(
            account_id.clone(),
            "unknown-exhausted",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("account should persist: {error}");
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };

        if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
            &async_store,
            &account_id,
            "responses",
            2_000,
        )
        .await
        {
            panic!("quota exhaustion should persist: {error}");
        }

        let inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
            &async_store,
            "responses",
            2_000,
        )
        .await
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        let input = inputs
            .iter()
            .find(|input| input.account_id() == &account_id)
            .unwrap_or_else(|| panic!("exhausted account selector input should exist"));
        assert_eq!(
            input
                .windows()
                .iter()
                .map(|window| window.limit_window_seconds())
                .collect::<Vec<_>>(),
            vec![18_000, 604_800]
        );
        assert!(
            input
                .windows()
                .iter()
                .all(|window| window.status() == SelectorQuotaWindowStatus::Ineligible),
            "suspect-exhausted accounts without prior quota windows must not degrade to unknown fallback"
        );
    }

    #[tokio::test]
    async fn async_quota_exhaustion_expires_back_to_probe_candidate() {
        let temp_dir = TestTempDir::new("async_quota_exhaustion_ttl");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_exhausted_ttl");
        let account =
            AccountRecord::new(account_id.clone(), "exhausted-ttl", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("account should persist: {error}");
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };

        if let Err(error) = AsyncQuotaExhaustionRepository::mark_route_band_quota_exhausted(
            &async_store,
            &account_id,
            "responses",
            2_000,
        )
        .await
        {
            panic!("quota exhaustion should persist: {error}");
        }

        let expired_inputs = match AsyncSelectorQuotaRepository::selector_inputs_for_route_band(
            &async_store,
            "responses",
            2_301,
        )
        .await
        {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load after suspect TTL expiry: {error}"),
        };
        let expired_input = expired_inputs
            .iter()
            .find(|input| input.account_id() == &account_id)
            .unwrap_or_else(|| panic!("expired suspect account selector input should exist"));
        assert!(
            expired_input.windows().is_empty(),
            "expired suspect-exhausted state should return to unknown probe behavior"
        );
    }

    #[test]
    fn refresh_success_replaces_selector_windows_and_records_status() {
        let temp_dir = TestTempDir::new("refresh_success_replaces_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_refresh_success");
        let account = AccountRecord::new(account_id.clone(), "refresh", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let old_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(99)
        .with_observed_unix_seconds(900);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &old_window) {
            panic!("old selector window should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(72)
        .with_reset_unix_seconds(19_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(44)
        .with_reset_unix_seconds(700_000)
        .with_observed_unix_seconds(1_000);

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                &[short_window.clone(), weekly_window.clone()],
                1_000,
                2_000,
            )
        {
            panic!("refresh success should persist atomically: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_500,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        assert_eq!(
            selector_inputs[0].windows(),
            &[short_window.clone(), weekly_window.clone()]
        );
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].account_id(), &account_id);
        assert_eq!(
            statuses[0].status_source(),
            QuotaRefreshStatusSource::Recorded
        );
        assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_error_class(), None);
        assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                std::slice::from_ref(&weekly_window),
                1_100,
                2_100,
            )
        {
            panic!("weekly-only refresh should replace the current pair: {error}");
        }
        let weekly_only_inputs =
            SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500)
                .unwrap_or_else(|error| panic!("weekly-only selector input should load: {error}"));
        assert_eq!(
            weekly_only_inputs[0].windows(),
            std::slice::from_ref(&weekly_window)
        );

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                &[short_window.clone(), weekly_window.clone()],
                1_200,
                2_200,
            )
        {
            panic!("returned short window should restore the current pair: {error}");
        }
        let restored_inputs =
            SelectorQuotaRepository::selector_inputs_for_route_band(&store, "responses", 1_500)
                .unwrap_or_else(|error| panic!("restored selector input should load: {error}"));
        assert_eq!(restored_inputs[0].windows(), &[short_window, weekly_window]);
    }

    #[test]
    fn refresh_failure_preserves_windows_and_overlays_stale_on_read() {
        let temp_dir = TestTempDir::new("refresh_failure_preserves_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_refresh_failure");
        let account = AccountRecord::new(account_id.clone(), "failure", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let short_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(61)
        .with_reset_unix_seconds(19_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) =
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &store,
                &account_id,
                "responses",
                std::slice::from_ref(&short_window),
                1_000,
                10_000,
            )
        {
            panic!("refresh success seed should persist: {error}");
        }

        if let Err(error) =
            SelectorQuotaRepository::record_refresh_failure_preserving_selector_windows(
                &store,
                &account_id,
                "responses",
                2_000,
                QuotaRefreshErrorClass::NetworkError,
            )
        {
            panic!("refresh failure should preserve windows: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            2_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(windows[0].remaining_headroom(), 61);
        assert_eq!(windows[0].reset_unix_seconds(), Some(19_000));
        assert_eq!(windows[0].observed_unix_seconds(), 1_000);
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].last_success_unix_seconds(), Some(1_000));
        assert_eq!(statuses[0].last_attempt_unix_seconds(), Some(2_000));
        assert_eq!(
            statuses[0].last_error_class(),
            Some(QuotaRefreshErrorClass::NetworkError)
        );
        assert_eq!(statuses[0].stale_after_unix_seconds(), Some(2_000));
    }

    #[test]
    fn legacy_selector_rows_without_refresh_status_are_stale_and_reported() {
        let temp_dir = TestTempDir::new("legacy_missing_refresh_status");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let alpha_id = account_id("acct_alpha_legacy");
        let beta_id = account_id("acct_beta_empty");
        if let Err(error) = AccountStateRepository::upsert_account(
            &store,
            &AccountRecord::new(alpha_id.clone(), "alpha", AccountStatus::Enabled),
        ) {
            panic!("alpha account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(
            &store,
            &AccountRecord::new(beta_id, "beta", AccountStatus::Enabled),
        ) {
            panic!("beta account should persist: {error}");
        }
        let legacy_window = PersistedSelectorQuotaWindow::new(
            alpha_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(88)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(1_000);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(&store, &legacy_window)
        {
            panic!("legacy selector window should persist: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_500,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 2);
        let alpha = selector_inputs
            .iter()
            .find(|input| input.account_id() == &alpha_id)
            .unwrap_or_else(|| panic!("alpha selector input should exist"));
        assert_eq!(alpha.windows().len(), 1);
        assert_eq!(
            alpha.windows()[0].status(),
            SelectorQuotaWindowStatus::Stale
        );
        assert_eq!(alpha.windows()[0].remaining_headroom(), 88);
        let statuses = match SelectorQuotaRepository::quota_refresh_statuses_for_route_band(
            &store,
            "responses",
        ) {
            Ok(statuses) => statuses,
            Err(error) => panic!("refresh statuses should load: {error}"),
        };
        assert_eq!(statuses.len(), 1);
        assert_eq!(statuses[0].account_id(), &alpha_id);
        assert_eq!(
            statuses[0].status_source(),
            QuotaRefreshStatusSource::LegacyMissingRefreshStatus
        );
        assert_eq!(statuses[0].last_success_unix_seconds(), None);
        assert_eq!(statuses[0].last_attempt_unix_seconds(), None);
        assert_eq!(statuses[0].last_error_class(), None);
        assert_eq!(statuses[0].stale_after_unix_seconds(), None);
    }

    #[test]
    fn v2_migration_backfills_selector_windows_from_existing_quota_snapshots() {
        let temp_dir = TestTempDir::new("v2_selector_backfill");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v2_database_with_quota_snapshot(&database_path);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v2 state store should migrate to current schema: {error}"),
        };

        assert_eq!(store.schema_version(), 13);
        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load after migration: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let input = &selector_inputs[0];
        assert_eq!(input.account_id(), &account_id("acct_v2_backfill"));
        assert_eq!(input.active_credential_generation(), Some(1));
        assert_eq!(input.windows().len(), 1);
        let window = &input.windows()[0];
        assert_eq!(window.status(), SelectorQuotaWindowStatus::Stale);
        assert_eq!(window.remaining_headroom(), 64);
        assert_eq!(window.reset_unix_seconds(), Some(2_000));
        assert_eq!(window.limit_window_seconds(), 18_000);
        assert!(window.effective());
        let expected_code_review_snapshot = PersistedQuotaSnapshot::new(
            account_id("acct_v2_backfill"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("code_review", 64)
        .with_reset_unix_seconds(2_000)
        .with_stale_penalty(false);
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id("acct_v2_backfill"),
                "code_review"
            ),
            Ok(Some(expected_code_review_snapshot))
        );
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[test]
    fn v3_migration_removes_legacy_code_review_selector_windows() {
        let temp_dir = TestTempDir::new("v3_code_review_selector_cleanup");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v3_database_with_legacy_code_review_selector_window(&database_path);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v3 state store should migrate to current schema: {error}"),
        };

        assert_eq!(store.schema_version(), 13);
        let responses_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("responses selector input should load: {error}"),
        };
        assert_eq!(responses_inputs.len(), 1);
        assert_eq!(responses_inputs[0].windows().len(), 1);
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            1_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[test]
    fn v6_migration_adds_reset_credits_without_losing_existing_quota() {
        let temp_dir = TestTempDir::new("v6_reset_credits_migration");
        let database_path = temp_dir.path().join("state.sqlite");
        create_v6_database_without_reset_credits(&database_path);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v6 state store should migrate to current schema: {error}"),
        };

        assert_eq!(store.schema_version(), 13);
        let account_id = account_id("acct_v6_reset_credits");
        let snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
            &store,
            &account_id,
            "responses",
        ) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("v6 quota snapshot should survive migration"),
            Err(error) => panic!("v6 quota snapshot should load after migration: {error}"),
        };
        assert_eq!(snapshot.remaining_headroom(), 42);
        assert_eq!(snapshot.reset_unix_seconds(), Some(2_000));
        assert_eq!(snapshot.reset_credits_available(), None);
    }

    #[test]
    fn credential_mutation_invalidates_response_backed_alias_family_atomically() {
        let temp_dir = TestTempDir::new("credential_mutation_invalidates_aliases");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_credential_mutation");
        let account = AccountRecord::new(account_id.clone(), "mutation", AccountStatus::Disabled);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
            "code_review",
        ] {
            let snapshot =
                PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                    .with_observed_unix_seconds(3_000)
                    .with_route_band(route_band, 88)
                    .with_reset_unix_seconds(4_000)
                    .with_stale_penalty(false);
            if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
                panic!("{route_band} snapshot should persist: {error}");
            }
        }
        let legacy_code_review_selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "code_review",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(77)
        .with_reset_unix_seconds(999_999)
        .with_effective(true)
        .with_observed_unix_seconds(12_345);
        if let Err(error) = SelectorQuotaRepository::upsert_selector_window(
            &store,
            &legacy_code_review_selector_window,
        ) {
            panic!("legacy code_review selector window should persist: {error}");
        }

        if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            7,
            AccountStatus::Enabled,
        ) {
            panic!("credential mutation should activate and invalidate atomically: {error}");
        }

        let loaded_account = match AccountStateRepository::load_account(&store, &account_id) {
            Ok(Some(account)) => account,
            Ok(None) => panic!("account should still exist"),
            Err(error) => panic!("account should load: {error}"),
        };
        assert_eq!(loaded_account.status(), AccountStatus::Enabled);
        assert_eq!(loaded_account.active_credential_generation(), Some(7));
        for route_band in [
            "responses",
            "models",
            "memories_trace_summarize",
            "responses_compact",
        ] {
            let snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id,
                route_band,
            ) {
                Ok(Some(snapshot)) => snapshot,
                Ok(None) => panic!("{route_band} stale marker should exist"),
                Err(error) => panic!("{route_band} stale marker should load: {error}"),
            };
            assert_eq!(snapshot.remaining_headroom(), 0);
            assert_eq!(snapshot.observed_unix_seconds(), 0);
            assert_eq!(snapshot.reset_unix_seconds(), None);
            assert!(snapshot.stale_penalty());
            let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
                &store, route_band, 3_000,
            ) {
                Ok(inputs) => inputs,
                Err(error) => {
                    panic!("{route_band} selector input should load after mutation: {error}")
                }
            };
            assert_eq!(selector_inputs.len(), 1);
            let windows = selector_inputs[0].windows();
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
            assert_eq!(windows[0].remaining_headroom(), 0);
            assert_eq!(windows[0].observed_unix_seconds(), 0);
            assert!(windows[0].effective());
        }
        let code_review_snapshot = match QuotaSnapshotRepository::load_snapshot_for_route_band(
            &store,
            &account_id,
            "code_review",
        ) {
            Ok(Some(snapshot)) => snapshot,
            Ok(None) => panic!("code_review stale marker should exist"),
            Err(error) => panic!("code_review stale marker should load: {error}"),
        };
        assert_eq!(code_review_snapshot.remaining_headroom(), 0);
        assert!(code_review_snapshot.stale_penalty());
        let code_review_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            3_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(code_review_inputs.len(), 1);
        assert!(code_review_inputs[0].windows().is_empty());
    }

    #[tokio::test]
    async fn async_credential_generation_activation_fails_when_account_was_disabled() {
        let temp_dir = TestTempDir::new("async_credential_activation_disabled_race");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_async_activation_disabled_race");
        let enabled_account =
            AccountRecord::new(account_id.clone(), "race", AccountStatus::Enabled)
                .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &enabled_account) {
            panic!("enabled account should persist: {error}");
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let disabled_account =
            AccountRecord::new(account_id.clone(), "race", AccountStatus::Disabled)
                .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &disabled_account) {
            panic!("disabled account should persist: {error}");
        }

        let activation_error = match async_store
            .activate_account_credential_generation_if_current_and_invalidate_quota(
                &account_id,
                1,
                2,
                AccountStatus::Enabled,
            )
            .await
        {
            Ok(()) => panic!("disabled account must not be re-enabled by stale refresh commit"),
            Err(error) => error,
        };

        assert_eq!(
            activation_error,
            StateStoreError::AccountConcurrentModification {
                account_id: account_id.as_str().to_owned()
            }
        );
        let loaded_account = match AccountStateRepository::load_account(&sync_store, &account_id) {
            Ok(Some(account)) => account,
            Ok(None) => panic!("account should still exist"),
            Err(error) => panic!("account should load after failed activation: {error}"),
        };
        assert_eq!(loaded_account.status(), AccountStatus::Disabled);
        assert_eq!(loaded_account.active_credential_generation(), Some(1));
    }

    #[tokio::test]
    async fn provider_rejection_disables_only_current_credential_generation() {
        let temp_dir = TestTempDir::new("provider_rejection_generation_guard");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_provider_rejection_generation_guard");
        let account = AccountRecord::new(
            account_id.clone(),
            "provider-rejection",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&sync_store, &account) {
            panic!("account should persist: {error}");
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        if let Err(error) = sync_store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            2,
            AccountStatus::Enabled,
        ) {
            panic!("new credential generation should activate: {error}");
        }

        let stale_generation_disabled = match async_store
            .disable_account_if_credential_generation_current(&account_id, 1)
            .await
        {
            Ok(disabled) => disabled,
            Err(error) => panic!("stale provider rejection should be checked: {error}"),
        };
        assert!(!stale_generation_disabled);
        let current_generation_disabled = match async_store
            .disable_account_if_credential_generation_current(&account_id, 2)
            .await
        {
            Ok(disabled) => disabled,
            Err(error) => panic!("current provider rejection should disable account: {error}"),
        };
        assert!(current_generation_disabled);
        let disabled_account = match AccountStateRepository::load_account(&sync_store, &account_id)
        {
            Ok(Some(account)) => account,
            Ok(None) => panic!("disabled account should still exist"),
            Err(error) => panic!("disabled account should load: {error}"),
        };
        assert_eq!(disabled_account.status(), AccountStatus::Disabled);
        assert_eq!(disabled_account.active_credential_generation(), Some(2));

        if let Err(error) = sync_store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            3,
            AccountStatus::Enabled,
        ) {
            panic!("new login generation should restore eligibility: {error}");
        }
        let restored_account = match AccountStateRepository::load_account(&sync_store, &account_id)
        {
            Ok(Some(account)) => account,
            Ok(None) => panic!("restored account should still exist"),
            Err(error) => panic!("restored account should load: {error}"),
        };
        assert_eq!(restored_account.status(), AccountStatus::Enabled);
        assert_eq!(restored_account.active_credential_generation(), Some(3));
    }

    #[test]
    fn credential_mutation_invalidates_selector_windows_atomically() {
        let temp_dir = TestTempDir::new("credential_mutation_invalidates_selector_windows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_selector_mutation");
        let account = AccountRecord::new(
            account_id.clone(),
            "selector-mutation",
            AccountStatus::Disabled,
        );
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(50)
        .with_effective(true)
        .with_observed_unix_seconds(9_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&store, &selector_window)
        {
            panic!("selector window should persist: {error}");
        }
        let weekly_selector_window = PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(99)
        .with_reset_unix_seconds(700_000)
        .with_effective(true)
        .with_observed_unix_seconds(9_000);
        if let Err(error) =
            SelectorQuotaRepository::upsert_selector_window(&store, &weekly_selector_window)
        {
            panic!("weekly selector window should persist: {error}");
        }

        if let Err(error) = store.activate_account_credential_generation_and_invalidate_quota(
            &account_id,
            2,
            AccountStatus::Enabled,
        ) {
            panic!("credential mutation should invalidate selector windows: {error}");
        }

        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "responses",
            9_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        let windows = selector_inputs[0].windows();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].limit_window_seconds(), 18_000);
        assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Ineligible);
        assert_eq!(windows[0].remaining_headroom(), 0);
        assert_eq!(windows[0].reset_unix_seconds(), None);
        assert_eq!(windows[0].observed_unix_seconds(), 0);
        assert!(windows[0].effective());
    }

    #[test]
    fn quota_snapshot_upsert_keeps_code_review_out_of_selector_projection() {
        let temp_dir = TestTempDir::new("code_review_status_only");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_code_review_status_only");
        let account = AccountRecord::new(account_id.clone(), "status-only", AccountStatus::Enabled)
            .with_active_credential_generation(1);
        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account should persist: {error}");
        }
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(3_000)
                .with_route_band("code_review", 88)
                .with_reset_unix_seconds(4_000)
                .with_stale_penalty(false);

        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
            panic!("code_review quota snapshot should persist: {error}");
        }

        assert_eq!(
            QuotaSnapshotRepository::load_snapshot_for_route_band(
                &store,
                &account_id,
                "code_review"
            ),
            Ok(Some(snapshot))
        );
        let selector_inputs = match SelectorQuotaRepository::selector_inputs_for_route_band(
            &store,
            "code_review",
            3_000,
        ) {
            Ok(inputs) => inputs,
            Err(error) => panic!("code_review selector input should load: {error}"),
        };
        assert_eq!(selector_inputs.len(), 1);
        assert!(selector_inputs[0].windows().is_empty());
    }

    #[test]
    fn corrupt_account_metadata_fails_closed_for_that_account_only() {
        let temp_dir = TestTempDir::new("corrupt_account");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let healthy_id = match AccountId::new("acct_healthy") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let corrupt_id = match AccountId::new("acct_corrupt") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };

        if let Err(error) = store.upsert_account(&AccountRecord::new(
            healthy_id.clone(),
            "healthy",
            AccountStatus::Enabled,
        )) {
            panic!("healthy account should persist: {error}");
        }
        if let Err(error) = store.insert_raw_account_for_test(
            corrupt_id.as_str(),
            "refresh-token-canary",
            "not-a-status",
        ) {
            panic!("corrupt fixture should persist: {error}");
        }

        let corrupt_error = match store.load_account(&corrupt_id) {
            Ok(account) => panic!("corrupt account should not load: {account:?}"),
            Err(error) => error,
        };

        assert!(matches!(
            corrupt_error,
            StateStoreError::CorruptAccount { .. }
        ));
        assert!(!format!("{corrupt_error:?}").contains("refresh-token-canary"));
        assert_eq!(
            store.load_account(&healthy_id),
            Ok(Some(AccountRecord::new(
                healthy_id,
                "healthy",
                AccountStatus::Enabled
            )))
        );
    }

    #[test]
    fn unsupported_schema_version_fails_closed_on_open() {
        let temp_dir = TestTempDir::new("unsupported_schema");
        let database_path = temp_dir.path().join("state.sqlite");
        let raw = match rusqlite::Connection::open(&database_path) {
            Ok(raw) => raw,
            Err(error) => panic!("raw sqlite should open: {error}"),
        };
        if let Err(error) = raw.pragma_update(None, "user_version", 999_i64) {
            panic!("schema fixture should persist: {error}");
        }
        drop(raw);

        let error = match SqliteStateStore::open(&database_path) {
            Ok(store) => panic!("unsupported schema should not open: {store:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            StateStoreError::UnsupportedSchemaVersion { version: 999 }
        );
    }

    #[test]
    fn state_repository_contracts_are_proxy_usable() {
        let temp_dir = TestTempDir::new("repository_contracts");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_contract");
        let account = AccountRecord::new(account_id.clone(), "contract", AccountStatus::Enabled);
        let snapshot =
            PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::MockEndpoint)
                .with_observed_unix_seconds(1_200)
                .with_route_band("responses", 33);
        let affinity_key = AffinityKey::new("response_previous");

        if let Err(error) = AccountStateRepository::upsert_account(&store, &account) {
            panic!("account repository should persist: {error}");
        }
        if let Err(error) = QuotaSnapshotRepository::upsert_snapshot(&store, &snapshot) {
            panic!("quota repository should persist: {error}");
        }
        if let Err(error) = AffinityRepository::pin_account(&store, &affinity_key, &account_id) {
            panic!("affinity repository should persist: {error}");
        }

        assert_eq!(
            AccountStateRepository::load_account(&store, &account_id),
            Ok(Some(account))
        );
        assert_eq!(
            QuotaSnapshotRepository::load_snapshot(&store, &account_id),
            Ok(Some(snapshot))
        );
        assert_eq!(
            AffinityRepository::load_pin(&store, &affinity_key),
            Ok(Some(account_id))
        );
    }

    #[test]
    fn previous_response_affinity_owner_repository_is_hash_only_and_route_scoped() {
        let temp_dir = TestTempDir::new("affinity_owner_hash_only");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('a');
        let responses_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_owner"),
            7,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            1_000,
        );
        let models_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_models"),
            9,
            RouteBand::Models,
            AffinitySourceTransport::WebSocket,
            1_100,
        );

        if let Err(error) =
            AffinityRepository::write_previous_response_owner(&store, &responses_owner)
        {
            panic!("responses affinity owner should persist: {error}");
        }
        if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &models_owner)
        {
            panic!("models affinity owner should persist: {error}");
        }

        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Found(responses_owner))
        );
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Models.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Found(models_owner))
        );
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &affinity_hash('b'),
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Missing)
        );
        assert_no_previous_response_id_in_affinity_owner_rows(&database_path, "resp_raw_canary");
    }

    #[tokio::test]
    async fn async_previous_response_affinity_owner_matches_sync_repository_projection() {
        let temp_dir = TestTempDir::new("async_affinity_owner_hash_only");
        let database_path = temp_dir.path().join("state.sqlite");
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('c');
        let responses_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_async_owner"),
            11,
            RouteBand::Responses,
            AffinitySourceTransport::WebSocket,
            1_200,
        );
        if let Err(error) =
            AffinityRepository::write_previous_response_owner(&sync_store, &responses_owner)
        {
            panic!("responses affinity owner should persist: {error}");
        }
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };

        let sync_lookup = match AffinityRepository::load_previous_response_owner(
            &sync_store,
            &hash,
            RouteBand::Responses.as_str(),
        ) {
            Ok(lookup) => lookup,
            Err(error) => panic!("sync affinity owner should load: {error}"),
        };
        let async_lookup = match AsyncAffinityRepository::load_previous_response_owner(
            &async_store,
            &hash,
            RouteBand::Responses.as_str(),
        )
        .await
        {
            Ok(lookup) => lookup,
            Err(error) => panic!("async affinity owner should load: {error}"),
        };
        let async_missing = match AsyncAffinityRepository::load_previous_response_owner(
            &async_store,
            &affinity_hash('d'),
            RouteBand::Responses.as_str(),
        )
        .await
        {
            Ok(lookup) => lookup,
            Err(error) => panic!("async missing affinity owner should load: {error}"),
        };

        assert_eq!(async_lookup, sync_lookup);
        assert_eq!(async_missing, PreviousResponseAffinityOwnerLookup::Missing);
    }

    #[tokio::test]
    async fn async_previous_response_affinity_owner_write_matches_sync_projection() {
        let temp_dir = TestTempDir::new("async_affinity_owner_write");
        let database_path = temp_dir.path().join("state.sqlite");
        let async_store = match AsyncSqliteStateStore::open(&database_path).await {
            Ok(store) => store,
            Err(error) => panic!("async state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('e');
        let responses_owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id("acct_async_owner_write"),
            12,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            1_300,
        );
        if let Err(error) =
            AsyncAffinityRepository::write_previous_response_owner(&async_store, &responses_owner)
                .await
        {
            panic!("async affinity owner should persist: {error}");
        }
        let sync_store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("sync state store should open after async write: {error}"),
        };
        let sync_lookup = match AffinityRepository::load_previous_response_owner(
            &sync_store,
            &hash,
            RouteBand::Responses.as_str(),
        ) {
            Ok(lookup) => lookup,
            Err(error) => panic!("sync affinity owner should load after async write: {error}"),
        };

        assert_eq!(
            sync_lookup,
            PreviousResponseAffinityOwnerLookup::Found(responses_owner)
        );
    }

    #[test]
    fn previous_response_affinity_owner_detects_ambiguous_rows_and_can_purge() {
        let temp_dir = TestTempDir::new("affinity_owner_ambiguous_purge");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let hash = affinity_hash('c');
        for account_id_value in ["acct_first", "acct_second"] {
            let owner = PreviousResponseAffinityOwnerRecord::new(
                hash.clone(),
                account_id(account_id_value),
                1,
                RouteBand::Responses,
                AffinitySourceTransport::HttpSse,
                2_000,
            );
            if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &owner) {
                panic!("affinity owner should persist: {error}");
            }
        }

        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Ambiguous)
        );

        if let Err(error) = AffinityRepository::purge_previous_response_owners(&store) {
            panic!("affinity owners should purge: {error}");
        }
        assert_eq!(
            AffinityRepository::load_previous_response_owner(
                &store,
                &hash,
                RouteBand::Responses.as_str()
            ),
            Ok(PreviousResponseAffinityOwnerLookup::Missing)
        );
    }

    #[test]
    fn state_repository_lists_accounts_in_selector_stable_order() {
        let temp_dir = TestTempDir::new("list_accounts");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let beta_id = account_id("acct_beta");
        let alpha_id = account_id("acct_alpha");
        let disabled_id = account_id("acct_disabled");
        let beta = AccountRecord::new(beta_id, "beta", AccountStatus::Enabled);
        let alpha = AccountRecord::new(alpha_id, "alpha", AccountStatus::Enabled);
        let disabled = AccountRecord::new(disabled_id, "disabled", AccountStatus::Disabled);

        if let Err(error) = AccountStateRepository::upsert_account(&store, &beta) {
            panic!("beta account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(&store, &alpha) {
            panic!("alpha account should persist: {error}");
        }
        if let Err(error) = AccountStateRepository::upsert_account(&store, &disabled) {
            panic!("disabled account should persist: {error}");
        }

        let accounts = match AccountStateRepository::list_accounts(&store) {
            Ok(accounts) => accounts,
            Err(error) => panic!("account repository should list accounts: {error}"),
        };

        assert_eq!(accounts, vec![alpha, beta, disabled]);
    }

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new(name: &str) -> Self {
            let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "codex-router-state-{name}-{}-{unique}",
                std::process::id()
            ));
            if let Err(error) = fs::create_dir(&path) {
                panic!(
                    "failed to create test directory {}: {error}",
                    path.display()
                );
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                panic!(
                    "failed to remove test directory {}: {error}",
                    self.path.display()
                );
            }
        }
    }

    fn account_id(value: &str) -> AccountId {
        match AccountId::new(value) {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        }
    }

    fn quota_history_observation(
        account_id: AccountId,
        route_band: &str,
        limit_window_seconds: u64,
        observed_unix_seconds: u64,
        remaining_headroom: u32,
        reset_unix_seconds: Option<u64>,
    ) -> PersistedQuotaHistoryObservation {
        let mut observation = PersistedQuotaHistoryObservation::new(
            account_id,
            "safe-label",
            route_band,
            limit_window_seconds,
            observed_unix_seconds,
            remaining_headroom,
        )
        .with_window_status(SelectorQuotaWindowStatus::Eligible)
        .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
        .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success);
        if let Some(reset_unix_seconds) = reset_unix_seconds {
            observation = observation.with_reset_unix_seconds(reset_unix_seconds);
        }

        observation
    }

    fn affinity_hash(character: char) -> AffinityKeyHash {
        match AffinityKeyHash::new(character.to_string().repeat(64)) {
            Ok(hash) => hash,
            Err(error) => panic!("affinity hash should parse: {error}"),
        }
    }

    fn assert_no_previous_response_id_in_affinity_owner_rows(
        database_path: &Path,
        raw_previous_response_id: &str,
    ) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw sqlite should open: {error}"),
        };
        let count: i64 = match connection.query_row(
            "SELECT COUNT(*)
               FROM previous_response_affinity_owners
              WHERE affinity_key_hash = ?1
                 OR route_band = ?1
                 OR account_id = ?1
                 OR source_transport = ?1",
            [raw_previous_response_id],
            |row| row.get(0),
        ) {
            Ok(count) => count,
            Err(error) => panic!("raw sqlite count should query: {error}"),
        };
        assert_eq!(count, 0);
    }

    fn create_v2_database_with_quota_snapshot(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v2 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
                account_id TEXT NOT NULL,
                source TEXT NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                route_band TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                stale_penalty INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            INSERT INTO accounts (
                account_id, label, status, active_credential_generation
            ) VALUES (
                'acct_v2_backfill', 'v2-backfill', 'enabled', 1
            );

            INSERT INTO quota_snapshots (
                account_id, source, observed_unix_seconds, route_band,
                remaining_headroom, reset_unix_seconds, stale_penalty
            ) VALUES (
                'acct_v2_backfill', 'mock_endpoint', 1000, 'responses',
                64, 2000, 0
            );

            INSERT INTO quota_snapshots (
                account_id, source, observed_unix_seconds, route_band,
                remaining_headroom, reset_unix_seconds, stale_penalty
            ) VALUES (
                'acct_v2_backfill', 'mock_endpoint', 1000, 'code_review',
                64, 2000, 0
            );

            PRAGMA user_version = 2;
            ",
        ) {
            panic!("raw v2 database should initialize: {error}");
        }
    }

    fn create_v3_database_with_legacy_code_review_selector_window(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v3 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
                account_id TEXT NOT NULL,
                source TEXT NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                route_band TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                stale_penalty INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            CREATE TABLE selector_quota_windows (
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

            INSERT INTO accounts (
                account_id, label, status, active_credential_generation
            ) VALUES (
                'acct_v3_cleanup', 'v3-cleanup', 'enabled', 1
            );

            INSERT INTO selector_quota_windows (
                account_id, route_band, limit_window_seconds, status,
                remaining_headroom, reset_unix_seconds, effective,
                observed_unix_seconds
            ) VALUES (
                'acct_v3_cleanup', 'responses', 18000, 'eligible',
                64, 2000, 1, 1000
            );

            INSERT INTO selector_quota_windows (
                account_id, route_band, limit_window_seconds, status,
                remaining_headroom, reset_unix_seconds, effective,
                observed_unix_seconds
            ) VALUES (
                'acct_v3_cleanup', 'code_review', 604800, 'eligible',
                77, 999999, 1, 12345
            );

            PRAGMA user_version = 3;
            ",
        ) {
            panic!("raw v3 database should initialize: {error}");
        }
    }

    fn create_v6_database_without_reset_credits(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v6 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
                account_id TEXT NOT NULL,
                source TEXT NOT NULL,
                observed_unix_seconds INTEGER NOT NULL,
                route_band TEXT NOT NULL,
                remaining_headroom INTEGER NOT NULL,
                reset_unix_seconds INTEGER,
                stale_penalty INTEGER NOT NULL,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            CREATE TABLE selector_quota_windows (
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

            CREATE TABLE quota_refresh_status (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                last_success_unix_seconds INTEGER,
                last_attempt_unix_seconds INTEGER,
                last_error_class TEXT,
                stale_after_unix_seconds INTEGER,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE previous_response_affinity_owners (
                affinity_key_hash TEXT NOT NULL,
                route_band TEXT NOT NULL,
                account_id TEXT NOT NULL,
                credential_generation INTEGER NOT NULL,
                source_transport TEXT NOT NULL,
                created_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (affinity_key_hash, route_band, account_id)
            );

            INSERT INTO accounts (
                account_id, label, status, active_credential_generation
            ) VALUES (
                'acct_v6_reset_credits', 'v6-reset-credits', 'enabled', 1
            );

            INSERT INTO quota_snapshots (
                account_id, source, observed_unix_seconds, route_band,
                remaining_headroom, reset_unix_seconds, stale_penalty
            ) VALUES (
                'acct_v6_reset_credits', 'mock_endpoint', 1_000, 'responses',
                42, 2_000, 0
            );

            PRAGMA user_version = 6;
            ",
        ) {
            panic!("raw v6 database should initialize: {error}");
        }
    }

    fn create_v8_database_with_current_active_lease(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v8 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
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

            CREATE TABLE affinity_pins (
                affinity_key TEXT PRIMARY KEY NOT NULL,
                account_id TEXT NOT NULL
            );

            CREATE TABLE selector_quota_windows (
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

            CREATE TABLE quota_refresh_status (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                last_success_unix_seconds INTEGER,
                last_attempt_unix_seconds INTEGER,
                last_error_class TEXT,
                stale_after_unix_seconds INTEGER,
                PRIMARY KEY (account_id, route_band)
            );

            CREATE TABLE previous_response_affinity_owners (
                affinity_key_hash TEXT NOT NULL,
                route_band TEXT NOT NULL,
                account_id TEXT NOT NULL,
                credential_generation INTEGER NOT NULL,
                source_transport TEXT NOT NULL,
                created_unix_seconds INTEGER NOT NULL,
                PRIMARY KEY (affinity_key_hash, route_band, account_id)
            );

            CREATE TABLE active_client_leases (
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                acquired_unix_seconds INTEGER NOT NULL,
                active_pressure INTEGER NOT NULL,
                PRIMARY KEY (route_band, process_run_id, reservation_id)
            );

            CREATE INDEX active_client_leases_account_lookup
                ON active_client_leases (
                    route_band, account_id, acquired_unix_seconds
                );

            INSERT INTO active_client_leases (
                route_band, process_run_id, reservation_id, account_id,
                acquired_unix_seconds, active_pressure
            ) VALUES (
                'responses', 'process-v8', 'reservation-v8',
                'acct_v8_active_lease', 100, 8
            );

            PRAGMA user_version = 8;
            ",
        ) {
            panic!("raw v8 database should initialize: {error}");
        }
    }

    fn create_v10_database_missing_async_projection_tables(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw v10 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL,
                active_credential_generation INTEGER
            );

            CREATE TABLE quota_snapshots (
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

            CREATE TABLE selector_quota_windows (
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

            CREATE TABLE quota_refresh_status (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                last_success_unix_seconds INTEGER,
                last_attempt_unix_seconds INTEGER,
                last_error_class TEXT,
                stale_after_unix_seconds INTEGER,
                PRIMARY KEY (account_id, route_band)
            );

            PRAGMA user_version = 10;
            ",
        ) {
            panic!("raw v10 database should initialize without async projection tables: {error}");
        }
    }

    fn create_partial_v10_database_missing_active_session_columns(database_path: &Path) {
        let connection = match Connection::open(database_path) {
            Ok(connection) => connection,
            Err(error) => panic!("raw partial v10 database should open: {error}"),
        };
        if let Err(error) = connection.execute_batch(
            "
            CREATE TABLE active_client_leases (
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                account_id TEXT NOT NULL,
                acquired_unix_seconds INTEGER NOT NULL,
                active_pressure INTEGER NOT NULL,
                PRIMARY KEY (route_band, process_run_id, reservation_id)
            );

            CREATE INDEX active_client_leases_account_lookup
                ON active_client_leases (
                    route_band, account_id, acquired_unix_seconds
                );

            CREATE TABLE active_session_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                process_run_id TEXT NOT NULL,
                reservation_id TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                event_unix_seconds INTEGER NOT NULL
            );

            CREATE INDEX active_session_events_route_lookup
                ON active_session_events (
                    route_band, account_id, event_unix_seconds
                );

            CREATE TABLE active_session_rollups (
                account_id TEXT NOT NULL,
                route_band TEXT NOT NULL,
                bucket_start_unix_seconds INTEGER NOT NULL,
                bucket_end_unix_seconds INTEGER NOT NULL,
                active_session_seconds INTEGER NOT NULL,
                max_concurrent_sessions INTEGER NOT NULL,
                PRIMARY KEY (
                    account_id,
                    route_band,
                    bucket_start_unix_seconds,
                    bucket_end_unix_seconds
                )
            );

            PRAGMA user_version = 10;
            ",
        ) {
            panic!("raw partial v10 database should initialize: {error}");
        }
    }
}
