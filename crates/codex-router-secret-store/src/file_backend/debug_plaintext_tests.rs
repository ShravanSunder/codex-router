//! Root-policy proof with fresh private fixtures and an observed Keychain boundary.
use super::*;
use crate::encrypted_credential_store::EncryptedCredentialStore;
use crate::keychain_data_key::{KeychainAccess, KeychainAccessError};
use std::sync::atomic::AtomicUsize;

#[derive(Default)]
struct CountingKeychain(AtomicUsize);
impl KeychainAccess for CountingKeychain {
    fn read_secret(&self, _: &str, _: &str) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(KeychainAccessError::Unavailable)
    }
    fn add_secret(&self, _: &str, _: &str, _: &[u8]) -> Result<(), KeychainAccessError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(KeychainAccessError::Unavailable)
    }
}

#[test]
fn declared_plaintext_and_malformed_markers_reject_encrypted_open_before_keychain() {
    for marker in [DECLARATION_VALUE, b"unsupported-mode\n"] {
        let directory = tempfile::tempdir().expect("fixture");
        fs::write(directory.path().join(DECLARATION_FILE), marker).expect("marker");
        let keychain = CountingKeychain::default();
        assert!(
            EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
                .is_err()
        );
        assert_eq!(keychain.0.load(Ordering::SeqCst), 0);
        assert!(!directory.path().join("store-id").exists());
        assert!(!directory.path().join("format-v2.marker").exists());
    }
}

#[cfg(not(debug_assertions))]
#[test]
fn release_declared_plaintext_root_is_rejected() {
    let directory = tempfile::tempdir().expect("fixture");
    fs::write(directory.path().join(DECLARATION_FILE), DECLARATION_VALUE).expect("marker");
    assert!(matches!(
        FileSecretStore::open_declared_debug_plaintext(directory.path()),
        Err(SecretStoreError::DebugPlaintextUnavailable)
    ));
}

#[cfg(debug_assertions)]
mod debug {
    use super::*;
    use crate::account_tokens::{AccountCredentialBundle, openai_account_credential_bundle_key};
    use crate::credential_bundle::CredentialBundle;
    use crate::runtime_credential_store::{RuntimeCredentialStore, RuntimeCredentialStoreStatus};
    use codex_router_core::ids::AccountId;
    use codex_router_core::provider::Provider;
    use std::os::unix::fs::PermissionsExt;

    fn credential_key(generation: u64) -> SecretKey {
        openai_account_credential_bundle_key(
            &AccountId::new("debug_fixture").expect("account"),
            generation,
        )
        .expect("key")
    }

    #[test]
    fn debug_plaintext_repeated_initialization_waits_for_active_atomic_write() {
        let directory = tempfile::tempdir().expect("fixture");
        let mut writer_store = FileSecretStore::initialize_debug_plaintext(directory.path())
            .expect("explicit root initialization");
        let trace = FileWriteTrace::default();
        writer_store.write_trace = Some(trace.clone());
        // The existing observer records after writing the real temp file and
        // before sync/rename. Holding its mutex retains that exact IO window.
        let trace_guard = trace.0.lock().expect("held atomic-write observer");
        let key = credential_key(1);
        let writer_key = key.clone();
        let bundle = AccountCredentialBundle::imported_codex_auth("overlap-access-canary", None);
        let payload = bundle.to_secret_string().expect("credential bundle");
        let writer_payload = payload.clone();
        let writer =
            std::thread::spawn(move || writer_store.write_secret(&writer_key, &writer_payload));
        let temp_prefix = format!(".{}.tmp.", key.as_str());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let active_temp = loop {
            let found = fs::read_dir(directory.path())
                .expect("fixture entries")
                .filter_map(Result::ok)
                .find(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(&temp_prefix)
                })
                .map(|entry| entry.path());
            if found.is_some() || std::time::Instant::now() >= deadline {
                break found;
            }
            std::thread::yield_now();
        };
        if active_temp.is_none() {
            drop(trace_guard);
            writer.join().expect("writer joined").expect("real write");
            panic!("actual pooled atomic temp was not observed");
        }
        // Only FileSecretStore::open changes this mode back to 0700. That
        // transition proves the initializer passed pre-lock validation while
        // the real writer still holds Shared and its atomic temp still exists.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750))
            .expect("initializer event baseline");
        let (result_sender, result_receiver) = std::sync::mpsc::channel();
        let root = directory.path().to_path_buf();
        let initializer = std::thread::spawn(move || {
            result_sender
                .send(FileSecretStore::initialize_debug_plaintext(&root))
                .expect("initializer result channel");
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut early_result = None;
        let reached_lock_boundary = loop {
            if let Ok(result) = result_receiver.try_recv() {
                early_result = Some(result);
                break false;
            }
            if fs::metadata(directory.path())
                .expect("root mode")
                .permissions()
                .mode()
                & 0o777
                == 0o700
            {
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            std::thread::yield_now();
        };
        if reached_lock_boundary {
            early_result = result_receiver
                .recv_timeout(std::time::Duration::from_millis(100))
                .ok();
        }
        let active_temp_existed = active_temp.as_ref().is_some_and(|path| path.exists());
        drop(trace_guard);
        let write_result = writer.join().expect("writer joined");
        let completed_while_write_held = early_result.is_some();
        let initialization_result = early_result.unwrap_or_else(|| {
            result_receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("initializer completed after writer release")
        });
        initializer.join().expect("initializer joined");
        let temp_removed = active_temp.as_ref().is_some_and(|path| !path.exists());
        eprintln!(
            "overlap cleanup: writer_joined=true; initializer_joined=true; temp_removed={temp_removed}"
        );
        write_result.expect("actual atomic write succeeded");
        assert!(
            initialization_result.is_ok(),
            "repeated opt-in must wait for the real active write: {initialization_result:?}"
        );
        assert!(reached_lock_boundary);
        assert!(
            !completed_while_write_held,
            "initializer returned while the writer held Shared"
        );
        assert!(active_temp_existed && temp_removed);
        let initialized = initialization_result.expect("repeated initialization succeeded");
        assert_eq!(
            initialized
                .read_secret(&key)
                .expect("credential roundtrip")
                .expose_secret(),
            payload.expose_secret()
        );
        assert_eq!(
            fs::read(directory.path().join(DECLARATION_FILE)).expect("declaration"),
            DECLARATION_VALUE
        );
        assert_eq!(
            fs::metadata(directory.path())
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(initialized.secret_path(&key))
                .expect("credential metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn encrypted_open_rechecks_declaration_after_waiting_for_exclusive_lock() {
        let directory = tempfile::tempdir().expect("fixture");
        let file_store = FileSecretStore::open(directory.path()).expect("file store");
        let lock =
            CredentialStoreLock::acquire(directory.path(), CredentialStoreLockMode::Exclusive)
                .expect("held setup lock");
        // Opening the file store sets 0700 after the first declaration check.
        // Observe that filesystem event before publishing the declaration while
        // retaining the lock; the opener must then reject it after acquisition.
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750))
            .expect("fixture event baseline");
        let keychain = std::sync::Arc::new(CountingKeychain::default());
        let child_keychain = keychain.clone();
        let root = directory.path().to_path_buf();
        let child = std::thread::spawn(move || {
            EncryptedCredentialStore::open_for_process_with_keychain(root, &*child_keychain)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fs::metadata(directory.path())
            .expect("root mode")
            .permissions()
            .mode()
            & 0o777
            != 0o700
        {
            assert!(
                std::time::Instant::now() < deadline,
                "opener did not pass its first declaration check"
            );
            std::thread::yield_now();
        }
        file_store
            .write_atomically(
                &directory.path().join(DECLARATION_FILE),
                DECLARATION_FILE,
                DECLARATION_VALUE,
            )
            .expect("declaration under the held lock");
        drop(lock);
        assert!(matches!(
            child.join().expect("opener joined"),
            Err(SecretStoreError::DebugPlaintextUnavailable)
        ));
        assert_eq!(keychain.0.load(Ordering::SeqCst), 0);
        assert!(!directory.path().join("store-id").exists());
    }

    #[test]
    fn debug_plaintext_bundle_roundtrip_is_atomic_private_and_reopens_without_opt_in() {
        let directory = tempfile::tempdir().expect("fixture");
        let mut file_store = FileSecretStore::initialize_debug_plaintext(directory.path())
            .expect("explicit initialization");
        let trace = FileWriteTrace::default();
        file_store.write_trace = Some(trace.clone());
        let store = RuntimeCredentialStore::DebugPlaintext(file_store);
        let bundle = CredentialBundle::OpenAi(AccountCredentialBundle::imported_codex_auth(
            "access-canary",
            Some("refresh-canary".to_owned()),
        ));
        let key = credential_key(1);
        let encoded = bundle.to_secret_string().expect("bundle serialization");
        store
            .write_staged(&key, &encoded)
            .expect("normal staged bundle path");
        let plaintext_path = directory.path().join(format!("{}.secret", key.as_str()));
        assert_eq!(
            fs::read_to_string(&plaintext_path).expect("actual plaintext file"),
            encoded.expose_secret()
        );
        assert!(
            !directory
                .path()
                .join(format!("{}.v2", key.as_str()))
                .exists()
        );
        let reopened = RuntimeCredentialStore::DebugPlaintext(
            FileSecretStore::open_declared_debug_plaintext(directory.path())
                .expect("reopen mode")
                .expect("declared root"),
        );
        assert_eq!(reopened.status(), RuntimeCredentialStoreStatus::Ready);
        let decoded = CredentialBundle::from_secret_string(
            Provider::Openai,
            reopened.read_secret(&key).expect("read staged bundle"),
        )
        .expect("decode bundle");
        assert_eq!(decoded, bundle);
        assert_eq!(
            fs::metadata(directory.path())
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        for path in [&plaintext_path, &directory.path().join(DECLARATION_FILE)] {
            assert_eq!(
                fs::metadata(path)
                    .expect("file metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        assert!(trace.events().iter().any(|event| matches!(event, FileWriteTraceEvent::TemporaryFileWritten { path, contents } if path.file_name().expect("name").to_string_lossy().starts_with(".openai_credential_bundle.") && contents == encoded.expose_secret().as_bytes())));
        assert!(trace.events().iter().any(|event| matches!(event, FileWriteTraceEvent::FileRenamed { to, .. } if to == &plaintext_path)));
        reopened.delete_staged(&key).expect("staged cleanup");
        reopened.delete_staged(&key).expect("idempotent cleanup");
        assert!(!plaintext_path.exists());
        FileSecretStore::initialize_debug_plaintext(directory.path()).expect("idempotent opt-in");
        assert!(
            fs::read_dir(directory.path())
                .expect("entries")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp."))
        );
    }

    #[test]
    fn debug_plaintext_pruning_retains_the_previous_generation_and_other_accounts() {
        let directory = tempfile::tempdir().expect("fixture");
        let store =
            FileSecretStore::initialize_debug_plaintext(directory.path()).expect("explicit root");
        for generation in 1..=4 {
            store
                .write_staged(&credential_key(generation), &SecretString::new("synthetic"))
                .expect("stage");
        }
        let other_account = AccountId::new("other_fixture").expect("account");
        let other_key = openai_account_credential_bundle_key(&other_account, 1).expect("key");
        store
            .write_staged(&other_key, &SecretString::new("other-synthetic"))
            .expect("other account");
        let removed = store
            .prune_obsolete_generations(
                Provider::Openai,
                &AccountId::new("debug_fixture").expect("account"),
                3,
            )
            .expect("prune");
        assert_eq!(removed, vec![1, 2]);
        assert!(store.read_secret(&credential_key(2)).is_err());
        assert!(store.read_secret(&credential_key(3)).is_ok());
        assert!(store.read_secret(&credential_key(4)).is_ok());
        assert!(store.read_secret(&other_key).is_ok());
    }

    #[test]
    fn default_file_store_still_refuses_pooled_operations_in_a_declared_root() {
        let directory = tempfile::tempdir().expect("fixture");
        FileSecretStore::initialize_debug_plaintext(directory.path()).expect("explicit root");
        let default = FileSecretStore::open(directory.path()).expect("general file store");
        assert!(matches!(
            default.write_secret(&credential_key(1), &SecretString::new("synthetic")),
            Err(SecretStoreError::PooledCredentialRequiresEncryption { .. })
        ));
        assert!(matches!(
            default.read_secret(&credential_key(1)),
            Err(SecretStoreError::PooledCredentialRequiresEncryption { .. })
        ));
        assert!(matches!(
            default.delete_staged(&credential_key(1)),
            Err(SecretStoreError::PooledCredentialRequiresEncryption { .. })
        ));
    }

    #[test]
    fn debug_plaintext_refuses_encrypted_layout_and_actual_atomic_temp_names() {
        for name in [
            "store-id",
            "format-v2.marker",
            "openai_credential_bundle.fixture.1.v2",
            ".store-id.tmp.23.7",
            ".format-v2.marker.tmp.23.7",
            ".openai_credential_bundle.fixture.1.tmp.23.7",
        ] {
            for declared in [false, true] {
                let directory = tempfile::tempdir().expect("fixture");
                if declared {
                    fs::write(directory.path().join(DECLARATION_FILE), DECLARATION_VALUE)
                        .expect("declaration");
                }
                fs::write(
                    directory.path().join(name),
                    b"uninspected synthetic content",
                )
                .expect("layout fixture");
                assert!(
                    matches!(
                        FileSecretStore::initialize_debug_plaintext(directory.path()),
                        Err(SecretStoreError::DebugPlaintextRootRejected)
                    ),
                    "{name}"
                );
                if declared {
                    assert!(
                        FileSecretStore::open_declared_debug_plaintext(directory.path()).is_err()
                    );
                }
                assert_eq!(
                    fs::read(directory.path().join(name)).expect("unchanged entry"),
                    b"uninspected synthetic content"
                );
            }
        }
    }

    #[test]
    fn debug_plaintext_refuses_malformed_marker_credential_key_and_symlinks() {
        let directory = tempfile::tempdir().expect("fixture");
        fs::write(directory.path().join(DECLARATION_FILE), b"invalid").expect("marker");
        assert!(matches!(
            FileSecretStore::initialize_debug_plaintext(directory.path()),
            Err(SecretStoreError::InvalidDebugPlaintextDeclaration)
        ));
        let directory = tempfile::tempdir().expect("fixture");
        let store = FileSecretStore::initialize_debug_plaintext(directory.path()).expect("root");
        let bad_key = SecretKey::new("openai_credential_bundle.fixture.0").expect("file key");
        assert!(matches!(
            store.write_staged(&bad_key, &SecretString::new("synthetic")),
            Err(SecretStoreError::InvalidCredentialKey { .. })
        ));
        let target = directory.path().join("external-synthetic");
        fs::write(&target, "untouched").expect("target");
        std::os::unix::fs::symlink(&target, store.secret_path(&credential_key(1)))
            .expect("credential symlink");
        assert!(store.read_secret(&credential_key(1)).is_err());
        assert!(
            store
                .write_secret(&credential_key(1), &SecretString::new("overwrite"))
                .is_err()
        );
        assert!(store.delete_staged(&credential_key(1)).is_err());
        assert_eq!(
            fs::read_to_string(target).expect("target unchanged"),
            "untouched"
        );
        let directory = tempfile::tempdir().expect("fixture");
        std::os::unix::fs::symlink(
            directory.path().join("missing"),
            directory.path().join(DECLARATION_FILE),
        )
        .expect("marker symlink");
        assert!(FileSecretStore::initialize_debug_plaintext(directory.path()).is_err());
        let alias = directory.path().join("root-alias");
        std::os::unix::fs::symlink(directory.path(), &alias).expect("root symlink");
        assert!(FileSecretStore::initialize_debug_plaintext(&alias).is_err());
    }

    #[test]
    fn debug_plaintext_revalidates_mode_and_layout_before_each_pooled_operation() {
        let directory = tempfile::tempdir().expect("fixture");
        let store = FileSecretStore::initialize_debug_plaintext(directory.path()).expect("root");
        store
            .write_staged(&credential_key(1), &SecretString::new("synthetic"))
            .expect("stage");
        fs::write(directory.path().join("store-id"), "synthetic-mixed-layout")
            .expect("mixed layout");
        assert!(store.read_secret(&credential_key(1)).is_err());
        assert!(
            store
                .write_secret(&credential_key(2), &SecretString::new("synthetic"))
                .is_err()
        );
        assert!(store.delete_staged(&credential_key(1)).is_err());
        let directory = tempfile::tempdir().expect("fixture");
        let store = FileSecretStore::initialize_debug_plaintext(directory.path()).expect("root");
        fs::write(directory.path().join(DECLARATION_FILE), "changed").expect("changed declaration");
        assert!(
            store
                .write_secret(&credential_key(1), &SecretString::new("synthetic"))
                .is_err()
        );
    }

    #[test]
    fn debug_plaintext_refuses_production_roots_and_canonical_aliases_without_mutation() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("home"));
        let protected = home.join(".codex-router");
        for path in [
            &protected,
            &protected.join("secrets"),
            &home,
            Path::new("/tmp"),
        ] {
            assert!(FileSecretStore::initialize_debug_plaintext(path).is_err());
        }
        let directory = tempfile::tempdir().expect("fixture");
        let alias = directory.path().join("production-alias");
        std::os::unix::fs::symlink(&protected, &alias).expect("alias");
        assert!(FileSecretStore::initialize_debug_plaintext(&alias.join("secrets")).is_err());
    }
}
