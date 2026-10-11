//! Secret storage boundary for codex-router.

pub mod account_tokens;
pub mod affinity_secret;
pub mod backend;
pub mod credential_bundle;
pub mod credential_key;
pub mod credential_migration;
pub mod credential_store_lock;
pub mod encrypted_credential_store;
pub mod file_backend;
pub mod keychain_data_key;
pub mod local_router_token;
pub mod model;
pub mod refresh_lease;
pub mod runtime_credential_store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
mod encrypted_credential_store_tests;

#[cfg(test)]
mod credential_migration_tests;

pub use backend::SecretStore;

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
    use std::sync::Arc;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::thread;

    use codex_router_core::ids::AccountId;
    use codex_router_core::provider::Provider;
    use codex_router_core::redaction::SecretString;

    use super::package_name;
    use crate::SecretStore;
    use crate::account_tokens::AccountCredentialBundle;
    use crate::account_tokens::upstream_access_token_key;
    use crate::affinity_secret::ROUTER_AFFINITY_HASH_SECRET_KEY;
    use crate::affinity_secret::RouterAffinityHashSecretOrigin;
    use crate::affinity_secret::load_or_create_router_affinity_hash_secret;
    use crate::affinity_secret::router_affinity_hash_secret_key;
    use crate::credential_key::AccountCredentialKey;
    use crate::file_backend::FileSecretStore;
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
    fn file_backend_refuses_pooled_credential_writes() {
        let test_root = TestRoot::new("plaintext-credential-write");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let account_id = must_ok(AccountId::new("acct_plaintext_write"));
        let key = must_ok(crate::account_tokens::openai_account_credential_bundle_key(
            &account_id,
            1,
        ));

        let result = store.write_secret(&key, &SecretString::new("credential-canary"));

        assert!(
            result.is_err(),
            "FileSecretStore must refuse pooled credential writes"
        );
        assert!(
            !test_root
                .path()
                .join(format!("{}.secret", key.as_str()))
                .exists()
        );
    }

    #[test]
    fn file_backend_refuses_legacy_pooled_credential_reads() {
        let test_root = TestRoot::new("plaintext-credential-read");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let account_id = must_ok(AccountId::new("acct_plaintext_read"));
        let key = must_ok(crate::account_tokens::openai_account_credential_bundle_key(
            &account_id,
            1,
        ));
        let plaintext_path = test_root.path().join(format!("{}.secret", key.as_str()));
        must_ok(fs::write(&plaintext_path, "credential-canary"));

        let result = store.read_secret(&key);

        assert!(
            result.is_err(),
            "FileSecretStore must refuse legacy pooled credential reads"
        );
    }

    #[test]
    fn openai_account_credential_bundle_key_includes_provider() {
        let account_id = must_ok(AccountId::new("acct_provider_key"));

        let openai_key = must_ok(crate::account_tokens::provider_credential_bundle_key(
            Provider::Openai,
            &account_id,
            1,
        ));
        let claude_key = must_ok(crate::account_tokens::provider_credential_bundle_key(
            Provider::Claude,
            &account_id,
            1,
        ));

        assert_eq!(
            openai_key.as_str(),
            "openai_credential_bundle.acct_provider_key.1"
        );
        assert_eq!(
            claude_key.as_str(),
            "claude_credential_bundle.acct_provider_key.1"
        );
    }

    #[test]
    fn openai_account_credential_bundle_key_rejects_zero_generation() {
        let account_id = must_ok(AccountId::new("acct_zero_generation"));

        let result = crate::account_tokens::openai_account_credential_bundle_key(&account_id, 0);

        assert!(
            result.is_err(),
            "generation zero is not a credential generation"
        );
    }

    #[test]
    fn keychain_service_apis_are_confined_to_the_keychain_access_boundary() {
        fn visit_rust_sources(path: &Path, sources: &mut Vec<PathBuf>) {
            let entries = fs::read_dir(path)
                .unwrap_or_else(|error| panic!("source directory should read: {error}"));
            for entry in entries {
                let entry =
                    entry.unwrap_or_else(|error| panic!("source entry should read: {error}"));
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    visit_rust_sources(&entry_path, sources);
                } else if entry_path
                    .extension()
                    .is_some_and(|extension| extension == "rs")
                {
                    sources.push(entry_path);
                }
            }
        }

        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        visit_rust_sources(&source_root, &mut sources);
        let security_framework_path = ["security_", "framework::"].concat();
        let apple_keychain_path = ["apple_native_", "keyring_store::keychain"].concat();
        for source_path in sources {
            let relative_path = source_path
                .strip_prefix(&source_root)
                .expect("source path should remain under the crate source root");
            if relative_path == Path::new("keychain_data_key.rs") {
                continue;
            }
            if relative_path == Path::new("keychain_data_key/temporary_keychain_tests.rs") {
                let keychain_module = fs::read_to_string(source_root.join("keychain_data_key.rs"))
                    .unwrap_or_else(|error| panic!("Keychain module should read: {error}"));
                assert!(keychain_module.contains(
                    "#[cfg(all(test, target_os = \"macos\"))]\n#[path = \"keychain_data_key/temporary_keychain_tests.rs\"]\npub(crate) mod temporary_keychain_tests;"
                ));
                continue;
            }
            let source = fs::read_to_string(&source_path)
                .unwrap_or_else(|error| panic!("Rust source should read: {error}"));
            assert!(
                !source.contains(&security_framework_path),
                "Security.framework access escaped the KeychainAccess module: {}",
                source_path.display()
            );
            assert!(
                !source.contains(&apple_keychain_path),
                "native Keychain calls escaped the KeychainAccess module: {}",
                source_path.display()
            );
        }
    }

    #[test]
    fn account_credential_key_parses_provider_account_and_generation() {
        let account_id = must_ok(AccountId::new("acct_parsed_key"));
        let secret_key = must_ok(crate::account_tokens::provider_credential_bundle_key(
            Provider::Claude,
            &account_id,
            17,
        ));

        let parsed = must_ok(AccountCredentialKey::parse(&secret_key));

        assert!(matches!(
            parsed,
            Some(key)
                if key.provider() == Provider::Claude
                    && key.account_id() == &account_id
                    && key.generation() == 17
        ));
    }

    #[test]
    fn account_credential_key_rejects_unknown_provider_and_zero_generation() {
        let unknown_provider = must_ok(SecretKey::new(
            "gemini_credential_bundle.acct_unknown_provider.1",
        ));
        let zero_generation = must_ok(SecretKey::new(
            "openai_credential_bundle.acct_zero_generation.0",
        ));

        assert!(matches!(
            AccountCredentialKey::parse(&unknown_provider),
            Err(crate::model::SecretStoreError::UnknownCredentialProvider {
                provider
            }) if provider == "gemini"
        ));
        assert!(matches!(
            AccountCredentialKey::parse(&zero_generation),
            Err(crate::model::SecretStoreError::InvalidCredentialKey { .. })
        ));
    }

    #[test]
    fn file_backend_read_only_open_requires_existing_root_and_preserves_mode() {
        let missing_root = TestRoot::new("read-only-missing");
        let error = must_err(FileSecretStore::open_read_only(missing_root.path()));
        assert!(error.to_string().contains("does not exist"));
        assert!(!missing_root.path().exists());

        let existing_root = TestRoot::new("read-only-existing");
        must_ok(fs::create_dir_all(existing_root.path()));
        set_mode(existing_root.path(), 0o750);

        let _store = must_ok(FileSecretStore::open_read_only(existing_root.path()));

        assert_eq!(mode(existing_root.path()), 0o750);
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
    fn router_affinity_hash_secret_is_created_once_and_redacted() {
        let test_root = TestRoot::new("affinity-secret");
        let store = must_ok(FileSecretStore::open(test_root.path()));

        let created = must_ok(load_or_create_router_affinity_hash_secret(&store));
        let loaded = must_ok(load_or_create_router_affinity_hash_secret(&store));

        assert_eq!(created.origin(), RouterAffinityHashSecretOrigin::CreatedNew);
        assert_eq!(
            loaded.origin(),
            RouterAffinityHashSecretOrigin::LoadedExisting
        );
        assert_eq!(
            created.secret().expose_secret(),
            loaded.secret().expose_secret()
        );
        assert_eq!(created.secret().expose_secret().len(), 64);
        assert!(
            created
                .secret()
                .expose_secret()
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(
            router_affinity_hash_secret_key()
                .unwrap_or_else(|error| panic!("affinity key should parse: {error}"))
                .as_str(),
            ROUTER_AFFINITY_HASH_SECRET_KEY
        );
        assert!(!format!("{created:?}").contains(created.secret().expose_secret()));
    }

    #[test]
    fn router_affinity_hash_secret_concurrent_create_returns_one_value() {
        let test_root = TestRoot::new("affinity-secret-concurrent");
        let store = Arc::new(must_ok(FileSecretStore::open(test_root.path())));
        let barrier = Arc::new(Barrier::new(8));
        let mut threads = Vec::new();

        for _worker_index in 0..8 {
            let worker_store = Arc::clone(&store);
            let worker_barrier = Arc::clone(&barrier);
            threads.push(thread::spawn(move || {
                worker_barrier.wait();
                let loaded = must_ok(load_or_create_router_affinity_hash_secret(
                    worker_store.as_ref(),
                ));
                loaded.secret().expose_secret().to_owned()
            }));
        }

        let mut loaded_values = Vec::new();
        for thread in threads {
            let value = match thread.join() {
                Ok(value) => value,
                Err(error) => panic!("affinity worker thread panicked: {error:?}"),
            };
            loaded_values.push(value);
        }

        let first = loaded_values
            .first()
            .unwrap_or_else(|| panic!("workers should produce values"));
        assert!(loaded_values.iter().all(|value| value == first));
        let loaded_after = must_ok(load_or_create_router_affinity_hash_secret(store.as_ref()));
        assert_eq!(loaded_after.secret().expose_secret(), first);
    }

    #[test]
    fn router_affinity_hash_secret_rejects_malformed_payload() {
        let test_root = TestRoot::new("affinity-secret-malformed");
        let store = must_ok(FileSecretStore::open(test_root.path()));
        let key = must_ok(router_affinity_hash_secret_key());
        must_ok(store.write_secret(&key, &SecretString::new("not-a-valid-secret")));

        let error = must_err(load_or_create_router_affinity_hash_secret(&store));

        assert!(error.to_string().contains("invalid secret payload"));
        assert!(!error.to_string().contains("not-a-valid-secret"));
    }

    #[test]
    fn imported_codex_auth_extracts_jwt_expiry_claim() {
        let bundle = AccountCredentialBundle::imported_codex_auth(
            "eyJhbGciOiJub25lIn0.eyJleHAiOjIwMDB9.sig",
            Some("refresh-token-canary".to_owned()),
        );

        assert_eq!(bundle.expires_unix_seconds(), Some(2_000));
    }

    #[test]
    fn empty_access_token_cannot_be_serialized_as_an_active_bundle() {
        let bundle = AccountCredentialBundle::imported_codex_auth(
            "   ",
            Some("refresh-token-canary".to_owned()),
        );
        let error = must_err(bundle.to_secret_string());
        assert!(matches!(
            error,
            crate::model::SecretStoreError::InvalidSecretPayload { .. }
        ));
        assert!(!error.to_string().contains("refresh-token-canary"));
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
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;

        must_ok(fs::set_permissions(path, fs::Permissions::from_mode(mode)));
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
pub mod account_credential_lock;
