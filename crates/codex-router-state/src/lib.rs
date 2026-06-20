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
    use crate::quota_snapshot::QuotaSnapshotSource;
    use crate::repositories::AccountStateRepository;
    use crate::repositories::AffinityRepository;
    use crate::repositories::QuotaSnapshotRepository;
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

        assert_eq!(store.schema_version(), 1);

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
}
