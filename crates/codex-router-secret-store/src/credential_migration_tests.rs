use fs2::FileExt;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;
use tempfile::TempDir;

use crate::account_tokens::openai_account_credential_bundle_key;
use crate::backend::SecretStore;
use crate::credential_migration::CredentialMigrationFailure;
use crate::credential_migration::CredentialMigrationOutcome;
use crate::credential_migration::migrate_pooled_credentials_at_startup;
use crate::credential_migration::migrate_with_exclusive_lock;
use crate::credential_store_lock::CredentialStoreLock;
use crate::credential_store_lock::CredentialStoreLockMode;
use crate::encrypted_credential_store::EncryptedCredentialStore;
use crate::encrypted_credential_store::EncryptedCredentialStoreStatus;
use crate::encrypted_credential_store::decrypt_envelope;
use crate::encrypted_credential_store::encrypt_envelope;
use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::KeychainAccessError;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::keychain_data_key::ROUTER_KEYCHAIN_SERVICE;
use crate::model::SecretKey;
use crate::test_support::FileWriteTrace;
use crate::test_support::FileWriteTraceEvent;

const ACCESS_TOKEN_CANARY: &str = "test-access-token-migration-canary";
const REFRESH_TOKEN_CANARY: &str = "test-refresh-token-migration-canary";

#[derive(Default)]
struct MemoryKeychainAccess {
    items: Mutex<HashMap<(String, String), Vec<u8>>>,
    available: AtomicBool,
}

impl MemoryKeychainAccess {
    fn new() -> Self {
        Self {
            items: Mutex::new(HashMap::new()),
            available: AtomicBool::new(true),
        }
    }

    fn set_available(&self, available: bool) {
        self.available.store(available, Ordering::SeqCst);
    }
}

impl KeychainAccess for MemoryKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        if !self.available.load(Ordering::SeqCst) {
            return Err(KeychainAccessError::Unavailable);
        }
        Ok(self
            .items
            .lock()
            .expect("memory Keychain lock")
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        if !self.available.load(Ordering::SeqCst) {
            return Err(KeychainAccessError::Unavailable);
        }
        let mut items = self.items.lock().expect("memory Keychain lock");
        let item_name = (service.to_owned(), account.to_owned());
        if items.contains_key(&item_name) {
            return Err(KeychainAccessError::Unavailable);
        }
        items.insert(item_name, secret.to_vec());
        Ok(())
    }
}

struct ServiceRejectedKeychainAccess;

impl KeychainAccess for ServiceRejectedKeychainAccess {
    fn read_secret(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        Err(KeychainAccessError::ServiceRejected)
    }

    fn add_secret(
        &self,
        _service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        Err(KeychainAccessError::ServiceRejected)
    }
}

fn test_credential_key(account_name: &str) -> SecretKey {
    let account_id = AccountId::new(account_name).expect("test account id");
    openai_account_credential_bundle_key(&account_id, 1).expect("test credential key")
}

fn test_data_key() -> PooledCredentialDataKey {
    PooledCredentialDataKey::from_bytes([0x5a; 32])
}

fn migrate_with_lock(
    file_store: &FileSecretStore,
    data_key: &PooledCredentialDataKey,
) -> CredentialMigrationOutcome {
    let _store_lock =
        CredentialStoreLock::acquire(file_store.root(), CredentialStoreLockMode::Exclusive)
            .expect("exclusive migration lock");
    migrate_with_exclusive_lock(file_store, data_key)
}

#[test]
fn migration_encrypts_reads_back_then_removes_legacy_source() {
    let directory = TempDir::new().expect("temporary root");
    let write_trace = FileWriteTrace::default();
    let file_store = FileSecretStore::open_with_write_trace(directory.path(), write_trace.clone())
        .expect("file store");
    let key = test_credential_key("acct_migration_success");
    let legacy_path = directory.path().join(format!("{}.secret", key.as_str()));
    let envelope_path = directory.path().join(format!("{}.v2", key.as_str()));
    std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy credential");

    let outcome = migrate_with_lock(&file_store, &test_data_key());

    assert_eq!(outcome, CredentialMigrationOutcome::Complete);
    assert!(!legacy_path.exists());
    assert!(envelope_path.exists());
    assert!(file_store.has_format_v2_marker().expect("format marker"));
    let envelope = file_store
        .read_credential_envelope(&key)
        .expect("encrypted envelope");
    assert!(!String::from_utf8_lossy(&envelope).contains(ACCESS_TOKEN_CANARY));
    let bundle = EncryptedCredentialStore::new(file_store, test_data_key());
    let decrypted = bundle.read_secret(&key).expect("encrypted read");
    assert_eq!(decrypted.expose_secret(), ACCESS_TOKEN_CANARY);

    let traced_temporary_write_contains_plaintext = write_trace.events().iter().any(|event| {
        matches!(event, FileWriteTraceEvent::TemporaryFileWritten { contents, .. }
            if contents.windows(ACCESS_TOKEN_CANARY.len()).any(|window| window == ACCESS_TOKEN_CANARY.as_bytes())
                || contents.windows(REFRESH_TOKEN_CANARY.len()).any(|window| window == REFRESH_TOKEN_CANARY.as_bytes()))
    });
    assert!(!traced_temporary_write_contains_plaintext);
    assert!(write_trace.events().iter().any(|event| {
        matches!(event, FileWriteTraceEvent::TemporaryFileWritten { path, .. }
            if path.file_name().is_some_and(|name| name.to_string_lossy().contains(".tmp.")))
    }));
    assert!(write_trace.events().iter().any(|event| {
        matches!(event, FileWriteTraceEvent::FileRenamed { to, .. } if to == &envelope_path)
    }));
    assert!(write_trace.events().iter().any(|event| {
        matches!(event, FileWriteTraceEvent::FileRemoved { path } if path == &legacy_path)
    }));
}

#[test]
fn corrupt_envelope_stops_migration_and_preserves_legacy_source() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let key = test_credential_key("acct_migration_corrupt");
    let legacy_path = directory.path().join(format!("{}.secret", key.as_str()));
    let encrypted_path = directory.path().join(format!("{}.v2", key.as_str()));
    std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy credential");
    std::fs::write(&encrypted_path, b"{ definitely not an envelope").expect("bad envelope");

    let outcome = migrate_with_lock(&file_store, &test_data_key());

    assert!(matches!(
        outcome,
        CredentialMigrationOutcome::Incomplete {
            failure: CredentialMigrationFailure::CredentialReadFailed,
            ..
        }
    ));
    assert!(legacy_path.exists());
    assert!(encrypted_path.exists());
    assert!(!directory.path().join("format-v2.marker").exists());
}

#[test]
fn mismatching_v2_and_legacy_files_are_both_preserved() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let data_key = test_data_key();
    let key = test_credential_key("acct_migration_mismatch");
    let legacy_path = directory.path().join(format!("{}.secret", key.as_str()));
    let envelope_path = directory.path().join(format!("{}.v2", key.as_str()));
    std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy credential");
    let wrong_envelope = encrypt_envelope(&data_key, &key, REFRESH_TOKEN_CANARY.as_bytes())
        .expect("wrong but authenticated envelope");
    file_store
        .write_credential_envelope(&key, &wrong_envelope)
        .expect("existing envelope");

    let outcome = migrate_with_lock(&file_store, &data_key);

    assert!(matches!(
        outcome,
        CredentialMigrationOutcome::Incomplete {
            failure: CredentialMigrationFailure::ReadBackMismatch,
            ..
        }
    ));
    assert!(legacy_path.exists());
    assert!(envelope_path.exists());
}

#[test]
fn encrypted_only_credential_without_marker_resumes_after_plaintext_deletion() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let data_key = test_data_key();
    let key = test_credential_key("acct_migration_deleted");
    let envelope = encrypt_envelope(&data_key, &key, ACCESS_TOKEN_CANARY.as_bytes())
        .expect("encrypted envelope");
    file_store
        .write_credential_envelope(&key, &envelope)
        .expect("encrypted generation");

    let outcome = migrate_with_lock(&file_store, &data_key);

    assert_eq!(outcome, CredentialMigrationOutcome::Complete);
    assert!(file_store.has_format_v2_marker().expect("format marker"));
    let decrypted = decrypt_envelope(
        &data_key,
        &key,
        &file_store
            .read_credential_envelope(&key)
            .expect("encrypted file"),
    )
    .expect("authenticated envelope");
    assert_eq!(decrypted.expose_secret(), ACCESS_TOKEN_CANARY);
}

#[test]
fn migration_recovers_every_persisted_crash_boundary() {
    // These on-disk states model crashes before rename, after rename, after verification,
    // after plaintext deletion, and during or after marker publication.
    let crash_boundaries = [
        "before-envelope-rename",
        "after-envelope-rename-before-readback",
        "after-readback-before-delete",
        "after-delete-before-marker",
        "during-marker-publication",
        "after-marker-publication",
    ];
    for boundary in crash_boundaries {
        let directory = TempDir::new().expect("temporary root");
        let file_store = FileSecretStore::open(directory.path()).expect("file store");
        let key = test_credential_key("acct_crash_matrix");
        let data_key = test_data_key();
        let legacy_path = directory.path().join(format!("{}.secret", key.as_str()));
        let encrypted_path = directory.path().join(format!("{}.v2", key.as_str()));
        let envelope = encrypt_envelope(&data_key, &key, ACCESS_TOKEN_CANARY.as_bytes())
            .expect("encrypted envelope");
        match boundary {
            "before-envelope-rename" => {
                std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy source");
                let temporary_path = directory
                    .path()
                    .join(format!(".{}.tmp.999.1", key.as_str()));
                std::fs::write(temporary_path, &envelope).expect("orphan encrypted temp");
            }
            "after-envelope-rename-before-readback" | "after-readback-before-delete" => {
                std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy source");
                file_store
                    .write_credential_envelope(&key, &envelope)
                    .expect("published envelope");
            }
            "after-delete-before-marker" => {
                file_store
                    .write_credential_envelope(&key, &envelope)
                    .expect("published envelope");
            }
            "during-marker-publication" => {
                file_store
                    .write_credential_envelope(&key, &envelope)
                    .expect("published envelope");
                std::fs::write(directory.path().join(".format-v2.marker.tmp.999.1"), b"2\n")
                    .expect("orphan marker temp");
            }
            "after-marker-publication" => {
                file_store
                    .write_credential_envelope(&key, &envelope)
                    .expect("published envelope");
                file_store
                    .write_format_v2_marker()
                    .expect("published marker");
            }
            _ => panic!("unknown migration crash boundary"),
        }

        let outcome = migrate_with_lock(&file_store, &data_key);

        assert_eq!(outcome, CredentialMigrationOutcome::Complete, "{boundary}");
        assert!(!legacy_path.exists(), "{boundary}");
        assert!(encrypted_path.exists(), "{boundary}");
        assert!(
            file_store.has_format_v2_marker().expect("format marker"),
            "{boundary}"
        );
        let all_entries = std::fs::read_dir(directory.path())
            .expect("list migrated files")
            .map(|entry| entry.expect("directory entry").file_name())
            .filter_map(|file_name| file_name.into_string().ok())
            .collect::<Vec<_>>();
        assert!(
            !all_entries
                .iter()
                .any(|file_name| file_name.ends_with(".tmp.999.1")),
            "{boundary}: stale temp file remains"
        );
    }
}

#[test]
fn missing_key_with_ciphertext_is_key_unavailable_and_never_recreated() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let key = test_credential_key("acct_key_missing");
    let envelope = encrypt_envelope(&test_data_key(), &key, ACCESS_TOKEN_CANARY.as_bytes())
        .expect("encrypted envelope");
    file_store
        .write_credential_envelope(&key, &envelope)
        .expect("encrypted credential");
    let keychain = MemoryKeychainAccess::new();

    let opened =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("serve still opens with unavailable credentials");

    assert_eq!(
        opened.status(),
        crate::encrypted_credential_store::EncryptedCredentialStoreStatus::KeyUnavailable
    );
    assert!(keychain.items.lock().expect("memory Keychain").is_empty());
    assert!(opened.read_secret(&key).is_err());
}

#[test]
fn invalid_stored_data_key_still_opens_a_key_unavailable_store() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let keychain = MemoryKeychainAccess::new();
    crate::keychain_data_key::load_or_create_pooled_credential_data_key(&file_store, &keychain)
        .expect("initial test key");
    let keychain_item = keychain
        .items
        .lock()
        .expect("memory Keychain")
        .keys()
        .next()
        .expect("one test keychain item")
        .clone();
    keychain
        .items
        .lock()
        .expect("memory Keychain")
        .insert(keychain_item, vec![0x54; 31]);

    let opened =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("serve opener should keep running with unavailable credentials");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
}

#[test]
fn rejected_keychain_service_still_opens_a_key_unavailable_store() {
    let directory = TempDir::new().expect("temporary root");

    let opened = EncryptedCredentialStore::open_for_process_with_keychain(
        directory.path(),
        &ServiceRejectedKeychainAccess,
    )
    .expect("serve opener should keep running after service rejection");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
}

#[test]
fn invalid_store_id_still_opens_a_key_unavailable_store() {
    let directory = TempDir::new().expect("temporary root");
    std::fs::write(directory.path().join("store-id"), "not-a-store-id")
        .expect("invalid store id fixture");

    let opened = EncryptedCredentialStore::open_for_process_with_keychain(
        directory.path(),
        &MemoryKeychainAccess::new(),
    )
    .expect("serve opener should keep running with invalid store metadata");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
}

#[test]
fn invalid_format_marker_still_opens_a_key_unavailable_store() {
    let directory = TempDir::new().expect("temporary root");
    std::fs::write(directory.path().join("format-v2.marker"), "unsupported")
        .expect("invalid format marker");

    let opened = EncryptedCredentialStore::open_for_process_with_keychain(
        directory.path(),
        &MemoryKeychainAccess::new(),
    )
    .expect("serve opener should keep running with an invalid marker");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
}

#[test]
fn unreadable_migration_metadata_preserves_its_failure_reason() {
    let directory = TempDir::new().expect("temporary root");
    std::fs::create_dir(
        directory
            .path()
            .join("openai_credential_bundle.acct_metadata.1.v2"),
    )
    .expect("unexpected directory entry");
    let keychain = MemoryKeychainAccess::new();

    let opened =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("serve opener should keep running with incomplete migration");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            accounts: Vec::new(),
            failure: CredentialMigrationFailure::InvalidStoreData,
        }
    );
}

#[test]
fn marker_read_failure_starts_router_with_migration_reason() {
    let directory = TempDir::new().expect("temporary root");
    std::fs::create_dir(directory.path().join("format-v2.marker"))
        .expect("unreadable marker directory");

    let opened = EncryptedCredentialStore::open_for_process_with_keychain(
        directory.path(),
        &MemoryKeychainAccess::new(),
    )
    .expect("serve opener should keep running when marker metadata cannot be read");

    assert_eq!(
        opened.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            accounts: Vec::new(),
            failure: CredentialMigrationFailure::MetadataReadFailed,
        }
    );
}

#[test]
fn denied_keychain_recovers_after_restart_without_login() {
    let directory = TempDir::new().expect("temporary root");
    let keychain = MemoryKeychainAccess::new();
    keychain.set_available(false);

    let first_process =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("serve starts while Keychain is denied");
    assert_eq!(
        first_process.status(),
        crate::encrypted_credential_store::EncryptedCredentialStoreStatus::KeyUnavailable
    );

    keychain.set_available(true);
    let migration = migrate_pooled_credentials_at_startup(directory.path(), &keychain)
        .expect("migration can access the Keychain after restart");
    assert_eq!(migration, CredentialMigrationOutcome::Complete);
    let restarted =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("restarted handle");

    assert_eq!(
        restarted.status(),
        crate::encrypted_credential_store::EncryptedCredentialStoreStatus::Ready
    );
}

#[test]
fn two_first_openers_publish_one_store_id_and_one_key() {
    let directory = TempDir::new().expect("temporary root");
    let keychain = Arc::new(MemoryKeychainAccess::new());
    let opener_barrier = Arc::new(Barrier::new(3));
    let mut openers = Vec::new();
    for _ in 0..2 {
        let secret_root = directory.path().to_path_buf();
        let keychain = Arc::clone(&keychain);
        let opener_barrier = Arc::clone(&opener_barrier);
        openers.push(thread::spawn(move || {
            opener_barrier.wait();
            EncryptedCredentialStore::open_for_process_with_keychain(
                &secret_root,
                keychain.as_ref(),
            )
            .expect("production opener should initialize the store")
        }));
    }
    opener_barrier.wait();
    let opened_stores = openers
        .into_iter()
        .map(|opener| opener.join().expect("first opener thread"))
        .collect::<Vec<_>>();

    assert_eq!(
        opened_stores[0].status(),
        EncryptedCredentialStoreStatus::Ready
    );
    assert_eq!(
        opened_stores[1].status(),
        EncryptedCredentialStoreStatus::Ready
    );
    assert_eq!(keychain.items.lock().expect("memory Keychain").len(), 1);
    assert!(directory.path().join("store-id").is_file());
    assert!(directory.path().join("format-v2.marker").is_file());
    let key = test_credential_key("acct_two_production_openers");
    opened_stores[0]
        .write_secret(&key, &SecretString::new("shared-opener-canary"))
        .expect("first opener should encrypt a credential");
    assert_eq!(
        opened_stores[1]
            .read_secret(&key)
            .expect("second opener should read with the shared key")
            .expose_secret(),
        "shared-opener-canary"
    );
}

#[test]
fn concurrent_login_and_renewal_writes_finish_before_exclusive_migration() {
    let directory = TempDir::new().expect("temporary root");
    let file_store = FileSecretStore::open(directory.path()).expect("file store");
    let data_key = test_data_key();
    let account_id = AccountId::new("acct_migration_writers").expect("account id");
    let legacy_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("legacy generation key");
    let login_key =
        openai_account_credential_bundle_key(&account_id, 2).expect("login generation key");
    let renewal_key =
        openai_account_credential_bundle_key(&account_id, 3).expect("renewal generation key");
    let legacy_path = directory
        .path()
        .join(format!("{}.secret", legacy_key.as_str()));
    std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("legacy fixture");
    let encrypted_store = EncryptedCredentialStore::new(file_store.clone(), data_key.clone());
    let writer_lock =
        CredentialStoreLock::acquire(directory.path(), CredentialStoreLockMode::Shared)
            .expect("shared login and renewal lock");
    let (migration_started_sender, migration_started_receiver) = std::sync::mpsc::channel();
    let (migration_result_sender, migration_result_receiver) = std::sync::mpsc::channel();
    let migration_root = directory.path().to_path_buf();
    let migration_key = data_key;
    let migration_thread = thread::spawn(move || {
        migration_started_sender
            .send(())
            .expect("migration thread should announce start");
        let migration_store = FileSecretStore::open(&migration_root).expect("migration file store");
        let result = migrate_with_lock(&migration_store, &migration_key);
        migration_result_sender
            .send(result)
            .expect("migration outcome should report");
    });
    migration_started_receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("migration thread should announce start");
    let lock_probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.path().join(".store.lock"))
        .expect("store lock probe should open");
    assert!(
        lock_probe.try_lock_exclusive().is_err(),
        "migration's exclusive lock must wait for shared login or renewal writers"
    );

    encrypted_store
        .write_staged(&login_key, &SecretString::new("login-generation-canary"))
        .expect("login generation should stage under shared lock");
    encrypted_store
        .write_staged(
            &renewal_key,
            &SecretString::new("renewal-generation-canary"),
        )
        .expect("renewal generation should stage under shared lock");
    assert!(!file_store.has_format_v2_marker().expect("marker check"));

    drop(writer_lock);
    assert_eq!(
        migration_result_receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("migration should proceed after shared writers finish"),
        CredentialMigrationOutcome::Complete
    );
    migration_thread
        .join()
        .expect("migration thread should finish");
    assert!(!legacy_path.exists());
    assert!(file_store.has_format_v2_marker().expect("migration marker"));
    assert_eq!(
        encrypted_store
            .read_secret(&login_key)
            .expect("login generation should remain encrypted")
            .expose_secret(),
        "login-generation-canary"
    );
    assert_eq!(
        encrypted_store
            .read_secret(&renewal_key)
            .expect("renewal generation should remain encrypted")
            .expose_secret(),
        "renewal-generation-canary"
    );
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires the opt-in unsandboxed temporary-Keychain test command"]
fn temporary_keychain_migrates_and_reopens_encrypted_credentials() {
    use crate::credential_migration::migrate_pooled_credentials_at_startup;
    use crate::keychain_data_key::temporary_keychain_tests::TemporaryKeychainAccess;
    use crate::keychain_data_key::temporary_keychain_tests::acquire_temporary_keychain_test_lock;

    assert!(matches!(
        std::env::var("CODEX_ROUTER_KEYCHAIN_TESTS").as_deref(),
        Ok("1")
    ));
    let _keychain_test_lock = acquire_temporary_keychain_test_lock();
    let directory = TempDir::new().expect("temporary router secret root");
    let keychain = TemporaryKeychainAccess::new().expect("temporary Keychain file");
    let key = test_credential_key("acct_temporary_keychain");
    let legacy_path = directory.path().join(format!("{}.secret", key.as_str()));
    std::fs::write(&legacy_path, ACCESS_TOKEN_CANARY).expect("fake legacy credential");

    let outcome = migrate_pooled_credentials_at_startup(directory.path(), &keychain)
        .expect("migration uses the explicit test Keychain");
    let first_process =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("first process store");
    let second_process =
        EncryptedCredentialStore::open_for_process_with_keychain(directory.path(), &keychain)
            .expect("second process store");

    assert_eq!(outcome, CredentialMigrationOutcome::Complete);
    assert!(keychain.path().is_absolute());
    assert_eq!(
        first_process.status(),
        crate::encrypted_credential_store::EncryptedCredentialStoreStatus::Ready
    );
    assert_eq!(
        second_process
            .read_secret(&key)
            .expect("reopened encrypted token")
            .expose_secret(),
        ACCESS_TOKEN_CANARY
    );
    assert!(!legacy_path.exists());
}
