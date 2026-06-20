//! Secret storage boundary for codex-router.

pub mod account_tokens;
pub mod file_backend;
pub mod model;
pub mod refresh_lease;

/// Returns this crate's package name.
#[must_use]
pub const fn package_name() -> &'static str {
    "codex-router-secret-store"
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use codex_router_core::ids::AccountId;
    use codex_router_core::redaction::SecretString;

    use super::package_name;
    use crate::account_tokens::upstream_access_token_key;
    use crate::file_backend::FileSecretStore;
    use crate::file_backend::SecretStore;
    use crate::model::SecretKey;
    use crate::refresh_lease::LeaseAcquisition;
    use crate::refresh_lease::ManualClock;
    use crate::refresh_lease::RefreshLeaseManager;

    static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new(name: &str) -> Self {
            let counter = TEMP_COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "codex-router-secret-store-{name}-{}-{counter}",
                std::process::id()
            ));
            if path.exists() {
                remove_dir_all(&path);
            }

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            if self.path.exists() {
                remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn reports_package_name() {
        assert_eq!(package_name(), "codex-router-secret-store");
    }

    #[test]
    fn file_backend_writes_private_root_and_secret_file() {
        let test_root = TestRoot::new("private");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let key = must_ok(SecretKey::new("local_router_token"));

        must_ok(store.write_secret(&key, &SecretString::new("secret-canary")));
        let secret = must_ok(store.read_secret(&key));

        assert_eq!(secret.expose_secret(), "secret-canary");
        assert_eq!(mode(test_root.path()), 0o700);
        assert_eq!(
            mode(&test_root.path().join("local_router_token.secret")),
            0o600
        );
    }

    #[test]
    fn file_backend_rejects_codex_home_and_symlink_paths() {
        let codex_root = TestRoot::new("codex-home");
        let codex_path = codex_root.path().join(".codex").join("router");
        let error = must_err(FileSecretStore::open(&codex_path));
        assert!(error.to_string().contains(".codex"));

        let real_root = TestRoot::new("real-root");
        must_ok(fs::create_dir_all(real_root.path()));
        let symlink_root = TestRoot::new("symlink-root");
        symlink_dir(real_root.path(), symlink_root.path());

        let error = must_err(FileSecretStore::open(symlink_root.path()));
        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn file_backend_rejects_symlink_secret_file_before_write() {
        let test_root = TestRoot::new("target-symlink");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let key = must_ok(SecretKey::new("oauth_refresh"));
        let external_file = test_root.path().join("external-secret");
        must_ok(fs::write(&external_file, "outside"));
        symlink_file(
            &external_file,
            &test_root.path().join("oauth_refresh.secret"),
        );

        let error = must_err(store.write_secret(&key, &SecretString::new("new-secret")));

        assert!(error.to_string().contains("symlink"));
    }

    #[test]
    fn secret_key_rejects_path_traversal() {
        for raw_key in ["", "../secret", "nested/secret", "bad key"] {
            let error = must_err(SecretKey::new(raw_key));
            assert!(error.to_string().contains("secret key"));
        }
    }

    #[test]
    fn upstream_access_token_key_is_namespaced_by_account_id() {
        let account_id = match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("account id should parse: {error}"),
        };
        let key = must_ok(upstream_access_token_key(&account_id));

        assert_eq!(key.as_str(), "openai_access_token.acct_primary");
    }

    #[test]
    fn refresh_lease_has_owner_follower_and_stale_recovery() {
        let clock = ManualClock::new(100);
        let manager = RefreshLeaseManager::new(clock.clone());

        let first = manager.acquire("quota:acct-a", "worker-a", 10);
        assert!(matches!(first, LeaseAcquisition::Acquired(_)));

        let second = manager.acquire("quota:acct-a", "worker-b", 10);
        assert!(matches!(
            second,
            LeaseAcquisition::Follower {
                owner,
                expires_at: 110
            } if owner == "worker-a"
        ));

        clock.advance(11);
        let third = manager.acquire("quota:acct-a", "worker-b", 10);
        assert!(matches!(third, LeaseAcquisition::Acquired(_)));
    }

    #[test]
    fn refresh_lease_finish_releases_only_matching_owner() {
        let clock = ManualClock::new(200);
        let manager = RefreshLeaseManager::new(clock);

        let first = match manager.acquire("quota:acct-b", "worker-a", 10) {
            LeaseAcquisition::Acquired(lease) => lease,
            LeaseAcquisition::Follower { .. } => panic!("first worker should own lease"),
        };
        let second = manager.acquire("quota:acct-b", "worker-b", 10);
        assert!(matches!(second, LeaseAcquisition::Follower { .. }));

        manager.finish(first);

        let third = manager.acquire("quota:acct-b", "worker-b", 10);
        assert!(matches!(third, LeaseAcquisition::Acquired(_)));
    }

    fn must_ok<T, E: std::fmt::Display>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected Ok, got error: {error}"),
        }
    }

    fn must_err<T, E>(result: Result<T, E>) -> E {
        match result {
            Ok(_) => panic!("expected Err, got Ok"),
            Err(error) => error,
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        must_ok(fs::metadata(path)).permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn symlink_dir(source: &Path, target: &Path) {
        must_ok(std::os::unix::fs::symlink(source, target));
    }

    #[cfg(unix)]
    fn symlink_file(source: &Path, target: &Path) {
        must_ok(std::os::unix::fs::symlink(source, target));
    }

    fn remove_dir_all(path: &Path) {
        if let Err(error) = fs::remove_dir_all(path) {
            panic!(
                "failed to remove test directory {}: {error}",
                path.display()
            );
        }
    }
}
