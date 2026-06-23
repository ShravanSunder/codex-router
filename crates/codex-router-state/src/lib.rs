//! SQLite-backed metadata boundary for codex-router.

pub mod account;
pub mod quota_snapshot;
pub mod repositories;
pub mod sqlite;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-state"
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use codex_router_core::ids::AccountId;
    use codex_router_core::ids::AffinityKey;

    use super::package_name;
    use crate::account::AccountRecord;
    use crate::account::AccountStatus;
    use crate::quota_snapshot::PersistedQuotaSnapshot;
    use crate::quota_snapshot::PersistedQuotaStatusRow;
    use crate::quota_snapshot::QuotaSnapshotSource;
    use crate::quota_snapshot::QuotaStatusState;
    use crate::repositories::AccountStateRepository;
    use crate::repositories::AffinityRepository;
    use crate::repositories::QuotaSnapshotRepository;
    use crate::repositories::QuotaStatusRepository;
    use crate::sqlite::SqliteStateStore;
    use crate::sqlite::StateStoreError;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-state");
    }

    #[test]
    fn sqlite_migration_roundtrips_account_and_quota_snapshot() {
        let temp_dir = TestTempDir::new("migration_roundtrip");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };

        assert_eq!(store.schema_version(), 2);

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
                .with_stale_penalty(false);
        if let Err(error) = store.upsert_quota_snapshot(&snapshot) {
            panic!("quota snapshot should persist: {error}");
        }

        assert_eq!(store.load_account(&account_id), Ok(Some(account)));
        assert_eq!(store.load_quota_snapshot(&account_id), Ok(Some(snapshot)));
    }

    #[test]
    fn sqlite_v1_database_migrates_to_v2_quota_status_schema() {
        let temp_dir = TestTempDir::new("migration_v1_to_v2");
        let database_path = temp_dir.path().join("state.sqlite");
        let raw = match rusqlite::Connection::open(&database_path) {
            Ok(raw) => raw,
            Err(error) => panic!("raw sqlite should open: {error}"),
        };
        if let Err(error) = raw.execute_batch(
            "
            CREATE TABLE accounts (
                account_id TEXT PRIMARY KEY NOT NULL,
                label TEXT NOT NULL,
                status TEXT NOT NULL
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
            PRAGMA user_version = 1;
            ",
        ) {
            panic!("v1 fixture should initialize: {error}");
        }
        drop(raw);

        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("v1 database should migrate to v2: {error}"),
        };
        assert_eq!(store.schema_version(), 2);

        let status = quota_status_row(account_id("acct_migrated"), "responses", "rate_limit", "5h")
            .with_status(QuotaStatusState::Fresh)
            .with_used_percent(25)
            .with_remaining_headroom(75)
            .with_effective(true);
        must_ok(QuotaStatusRepository::upsert_status_row(&store, &status));

        assert_eq!(
            QuotaStatusRepository::list_status_rows(&store),
            Ok(vec![status])
        );
    }

    #[test]
    fn sqlite_state_store_rejects_codex_home_path() {
        let temp_dir = TestTempDir::new("codex_home_state_path");
        let codex_home = temp_dir.path().join(".codex");
        must_create_dir(&codex_home);
        let database_path = codex_home.join("state.sqlite");

        let error = match SqliteStateStore::open(&database_path) {
            Ok(store) => panic!("state store must reject .codex path, got {store:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            StateStoreError::CodexHomePath {
                path: database_path
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn sqlite_state_store_rejects_symlink_parent_path() {
        use std::os::unix::fs::symlink;

        let temp_dir = TestTempDir::new("symlink_state_path");
        let real_root = temp_dir.path().join("real-root");
        let linked_root = temp_dir.path().join("linked-root");
        must_create_dir(&real_root);
        if let Err(error) = symlink(&real_root, &linked_root) {
            panic!("test symlink should create: {error}");
        }
        let database_path = linked_root.join("state.sqlite");

        let error = match SqliteStateStore::open(&database_path) {
            Ok(store) => panic!("state store must reject symlink parent, got {store:?}"),
            Err(error) => error,
        };

        assert_eq!(error, StateStoreError::SymlinkPath { path: linked_root });
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
    fn quota_status_rows_roundtrip_in_display_order() {
        let temp_dir = TestTempDir::new("quota_status_rows");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let account_id = account_id("acct_status_rows");
        let effective =
            quota_status_row(account_id.clone(), "responses", "rate_limit", "effective")
                .with_observed_unix_seconds(1_000)
                .with_status(QuotaStatusState::Fresh)
                .with_used_percent(80)
                .with_remaining_headroom(20)
                .with_reset_unix_seconds(9_000)
                .with_limit_window_seconds(604_800)
                .with_effective(true);
        let detailed = quota_status_row(account_id, "responses", "rate_limit", "5h")
            .with_observed_unix_seconds(1_000)
            .with_status(QuotaStatusState::Fresh)
            .with_used_percent(25)
            .with_remaining_headroom(75)
            .with_reset_unix_seconds(1_800)
            .with_limit_window_seconds(18_000);

        must_ok(QuotaStatusRepository::upsert_status_row(&store, &detailed));
        must_ok(QuotaStatusRepository::upsert_status_row(&store, &effective));

        assert_eq!(
            QuotaStatusRepository::list_status_rows(&store),
            Ok(vec![effective, detailed])
        );
    }

    #[test]
    fn corrupt_quota_status_percentage_fails_closed() {
        let temp_dir = TestTempDir::new("corrupt_quota_status_percent");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        must_ok(store.insert_raw_quota_status_for_test(
            "acct_corrupt_status",
            "mock_endpoint",
            1_000,
            "responses",
            "rate_limit",
            "5h",
            "fresh",
            Some(150),
            75,
            0,
        ));

        let error = match QuotaStatusRepository::list_status_rows(&store) {
            Ok(rows) => panic!("corrupt status row should fail closed, got {rows:?}"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            StateStoreError::CorruptQuotaStatus {
                account_id: "acct_corrupt_status".to_owned(),
                field: "used_percent",
            }
        );
    }

    #[test]
    fn route_quota_state_replacement_commits_selector_and_status_rows_atomically() {
        let temp_dir = TestTempDir::new("route_quota_atomic");
        let database_path = temp_dir.path().join("state.sqlite");
        let store = match SqliteStateStore::open(&database_path) {
            Ok(store) => store,
            Err(error) => panic!("state store should open and migrate: {error}"),
        };
        let atomic_account_id = account_id("acct_atomic");
        let old_snapshot = PersistedQuotaSnapshot::new(
            atomic_account_id.clone(),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("responses", 10);
        let old_status =
            quota_status_row(atomic_account_id.clone(), "responses", "rate_limit", "old")
                .with_observed_unix_seconds(1_000)
                .with_remaining_headroom(10);
        must_ok(QuotaStatusRepository::replace_route_quota_state(
            &store,
            &old_snapshot,
            std::slice::from_ref(&old_status),
        ));

        let new_snapshot = PersistedQuotaSnapshot::new(
            atomic_account_id.clone(),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(2_000)
        .with_route_band("responses", 90);
        let bad_status =
            quota_status_row(account_id("acct_other"), "responses", "rate_limit", "bad")
                .with_observed_unix_seconds(2_000)
                .with_remaining_headroom(90);
        let error = match QuotaStatusRepository::replace_route_quota_state(
            &store,
            &new_snapshot,
            &[bad_status],
        ) {
            Ok(()) => panic!("mismatched status row should rollback transaction"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            StateStoreError::CorruptQuotaStatus {
                field: "account_id_mismatch",
                ..
            }
        ));
        assert_eq!(
            store.load_quota_snapshot(&atomic_account_id),
            Ok(Some(old_snapshot))
        );
        assert_eq!(
            QuotaStatusRepository::list_status_rows(&store),
            Ok(vec![old_status])
        );
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

    fn quota_status_row(
        account_id: AccountId,
        route_band: &str,
        family: &str,
        window_label: &str,
    ) -> PersistedQuotaStatusRow {
        PersistedQuotaStatusRow::new(
            account_id,
            QuotaSnapshotSource::MockEndpoint,
            route_band,
            family,
            window_label,
        )
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }

    fn must_create_dir(path: &Path) {
        if let Err(error) = fs::create_dir(path) {
            panic!("failed to create directory {}: {error}", path.display());
        }
    }
}
