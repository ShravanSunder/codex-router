#![cfg(test)]

use std::fs;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use codex_router_core::ids::AccountId;
use codex_router_core::redaction::SecretString;

use crate::SecretStore;
use crate::account_tokens::openai_account_credential_bundle_key;
use crate::affinity_secret::RouterAffinityHashSecretOrigin;
use crate::affinity_secret::load_existing_router_affinity_hash_secret;
use crate::affinity_secret::load_or_create_router_affinity_hash_secret;
use crate::encrypted_credential_store::EncryptedCredentialStore;
use crate::encrypted_credential_store::EncryptedCredentialStoreStatus;
use crate::existing_proxy_secrets_test_support::RecordingKeychainAccess;
use crate::existing_proxy_secrets_test_support::assert_root_unchanged;
use crate::existing_proxy_secrets_test_support::seed_fresh_data_key;
use crate::existing_proxy_secrets_test_support::seed_fresh_ready_store;
use crate::existing_proxy_secrets_test_support::snapshot_root;
use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::KeychainAccessError;
use crate::keychain_data_key::load_existing_pooled_credential_data_key;
use crate::local_router_token::LocalRouterTokenService;
use crate::model::CredentialMigrationFailure;
use crate::model::SecretStoreError;

#[test]
fn existing_key_loader_uses_noninteractive_read_and_leaves_root_unchanged() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    let expected_data_key = seed_fresh_data_key(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture key setup failed"));
    let before = snapshot_root(&secret_root);
    let file_store = FileSecretStore::open_read_only(&secret_root)
        .unwrap_or_else(|_| panic!("existing secret root should open read-only"));

    let loaded_data_key = load_existing_pooled_credential_data_key(&file_store, &keychain)
        .unwrap_or_else(|_| panic!("existing data key should load"));

    assert!(
        loaded_data_key.bytes() == expected_data_key.bytes(),
        "loaded key differs"
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
    assert!(!secret_root.join(".store.lock").exists());
}

#[test]
fn missing_store_id_is_refused_without_publishing_an_id_or_reading_keychain() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    FileSecretStore::open(&secret_root).unwrap_or_else(|_| panic!("fixture root should open"));
    let keychain = RecordingKeychainAccess::new();
    keychain.allow_fixture_adds();
    keychain.stop_fixture_adds_and_reset_counts();
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("missing id should retain the current unavailable status"));

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 0);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn malformed_store_id_is_unavailable_without_keychain_reads_or_file_changes() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    fs::write(secret_root.join("store-id"), "not-a-store-id")
        .unwrap_or_else(|_| panic!("invalid store id fixture should write"));
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("invalid store id should keep an unavailable status"));

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 0);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn existing_open_does_not_create_a_lock_for_a_ready_empty_v2_store() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    let before = snapshot_root(&secret_root);
    assert!(!secret_root.join(".store.lock").exists());

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("existing ready store should open"));

    assert_eq!(store.status(), EncryptedCredentialStoreStatus::Ready);
    assert!(!secret_root.join(".store.lock").exists());
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn existing_open_reads_an_encrypted_credential_without_mutating_the_root() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    let data_key = seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    let file_store = FileSecretStore::open(&secret_root)
        .unwrap_or_else(|_| panic!("fixture store should remain open"));
    let setup_store = EncryptedCredentialStore::new(file_store, data_key);
    let account_id = AccountId::new("acct_existing_proxy_secret").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let expected_secret = SecretString::new("fixture-existing-secret");
    setup_store
        .write_staged(&credential_key, &expected_secret)
        .unwrap_or_else(|_| panic!("fixture credential should encrypt"));
    let before = snapshot_root(&secret_root);

    let existing_store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("existing encrypted store should open"));
    let loaded_secret = existing_store
        .read_secret(&credential_key)
        .unwrap_or_else(|_| panic!("existing encrypted credential should decrypt"));

    assert!(
        loaded_secret.expose_secret() == expected_secret.expose_secret(),
        "loaded credential differs"
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn missing_secret_root_is_not_created_by_existing_open() {
    let test_root = tempfile::tempdir().expect("isolated parent root");
    let secret_root = test_root.path().join("missing-secrets");
    let keychain = RecordingKeychainAccess::new();

    let result =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain);

    assert!(matches!(result, Err(SecretStoreError::Filesystem { .. })));
    assert!(!secret_root.exists());
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 0);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
}

#[test]
fn missing_key_with_existing_ciphertext_is_unavailable_without_add_or_file_changes() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    let account_id = AccountId::new("acct_missing_existing_key").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    fs::write(
        secret_root.join(format!("{}.v2", credential_key.as_str())),
        b"existing ciphertext fixture",
    )
    .unwrap_or_else(|_| panic!("ciphertext fixture should write"));
    keychain.remove_all_items();
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("existing store should keep an unavailable status"));

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn invalid_existing_data_key_is_unavailable_without_repair_or_file_changes() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    keychain.replace_all_item_bytes(vec![0x53; 31]);
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("invalid existing key should map to unavailable status"));

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn denied_noninteractive_keychain_read_is_unavailable_without_interactive_fallback() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    keychain.set_noninteractive_reads_available(false);
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("denied read should map to unavailable status"));

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::KeyUnavailable
    );
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn missing_v2_marker_is_incomplete_without_marker_publication() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_data_key(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture data-key setup failed"));
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("marker absence should retain a migration status"));

    assert!(matches!(
        store.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            failure: CredentialMigrationFailure::MigrationNotComplete,
            ..
        }
    ));
    assert!(!secret_root.join("format-v2.marker").exists());
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn invalid_v2_marker_is_incomplete_without_marker_repair() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_data_key(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture data-key setup failed"));
    fs::write(secret_root.join("format-v2.marker"), b"unsupported")
        .unwrap_or_else(|_| panic!("invalid marker fixture should write"));
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("invalid marker should retain migration status"));

    assert!(matches!(
        store.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            failure: CredentialMigrationFailure::InvalidStoreData,
            ..
        }
    ));
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn legacy_credential_debt_is_incomplete_without_legacy_deletion() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    let account_id = AccountId::new("acct_legacy_debt").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let legacy_path = secret_root.join(format!("{}.secret", credential_key.as_str()));
    fs::write(&legacy_path, b"legacy fixture bytes")
        .unwrap_or_else(|_| panic!("legacy fixture should write"));
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("legacy debt should retain migration status"));

    assert!(matches!(
        store.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            failure: CredentialMigrationFailure::InvalidStoreData,
            ..
        }
    ));
    assert!(legacy_path.is_file());
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn credential_temporary_debt_is_incomplete_without_cleanup() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let keychain = RecordingKeychainAccess::new();
    seed_fresh_ready_store(&secret_root, &keychain)
        .unwrap_or_else(|_| panic!("fixture store setup failed"));
    let account_id = AccountId::new("acct_temporary_debt").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let temporary_path = secret_root.join(format!(".{}.tmp.123.1", credential_key.as_str()));
    fs::write(&temporary_path, b"temporary fixture bytes")
        .unwrap_or_else(|_| panic!("temporary fixture should write"));
    let before = snapshot_root(&secret_root);

    let store =
        EncryptedCredentialStore::open_existing_for_process_with_keychain(&secret_root, &keychain)
            .unwrap_or_else(|_| panic!("temporary debt should retain migration status"));

    assert!(matches!(
        store.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            failure: CredentialMigrationFailure::UnexpectedEntry,
            ..
        }
    ));
    assert!(temporary_path.is_file());
    let calls = keychain.call_counts();
    assert_eq!(calls.noninteractive_reads, 1);
    assert_eq!(calls.interactive_reads, 0);
    assert_eq!(calls.add_attempts, 0);
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn existing_affinity_loader_reads_and_validates_without_writing() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let setup_store = FileSecretStore::open(&secret_root).expect("fixture store");
    let created =
        load_or_create_router_affinity_hash_secret(&setup_store).expect("fixture affinity secret");
    let read_trace = crate::file_backend::FileReadTrace::default();
    let write_trace = crate::file_backend::FileWriteTrace::default();
    let traced_store = FileSecretStore::open_with_read_and_write_traces(
        &secret_root,
        read_trace.clone(),
        write_trace.clone(),
    )
    .expect("existing traced store");
    let before = snapshot_root(&secret_root);

    let loaded = load_existing_router_affinity_hash_secret(&traced_store)
        .expect("existing affinity secret should load");

    assert_eq!(
        loaded.origin(),
        RouterAffinityHashSecretOrigin::LoadedExisting
    );
    assert!(loaded.secret().expose_secret() == created.secret().expose_secret());
    assert!(!read_trace.events().is_empty());
    assert!(
        write_trace.events().is_empty(),
        "existing affinity read wrote files"
    );
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn missing_affinity_secret_is_refused_without_randomness_or_file_creation() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    FileSecretStore::open(&secret_root).expect("fixture store");
    let store = FileSecretStore::open_read_only(&secret_root).expect("existing store");
    let before = snapshot_root(&secret_root);

    let result = load_existing_router_affinity_hash_secret(&store);

    assert!(matches!(result, Err(SecretStoreError::Filesystem { .. })));
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn malformed_affinity_secret_is_refused_without_repair() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let setup_store = FileSecretStore::open(&secret_root).expect("fixture store");
    let affinity_key =
        crate::affinity_secret::router_affinity_hash_secret_key().expect("affinity key");
    setup_store
        .write_secret(
            &affinity_key,
            &SecretString::new("malformed affinity fixture"),
        )
        .expect("malformed affinity fixture should write");
    let store = FileSecretStore::open_read_only(&secret_root).expect("existing store");
    let before = snapshot_root(&secret_root);

    let result = load_existing_router_affinity_hash_secret(&store);

    assert!(matches!(
        result,
        Err(SecretStoreError::InvalidSecretPayload { .. })
    ));
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn existing_local_router_token_reads_through_existing_root_without_writes() {
    let test_root = tempfile::tempdir().expect("isolated secret root");
    let secret_root = test_root.path().join("secrets");
    let setup_store = FileSecretStore::open(&secret_root).expect("fixture store");
    let expected = LocalRouterTokenService::new(setup_store)
        .rotate_with_token("fixture-local-router-token")
        .expect("fixture token");
    let before = snapshot_root(&secret_root);

    let read_only_store = FileSecretStore::open_read_only(&secret_root)
        .expect("existing store should open read-only");
    let loaded = LocalRouterTokenService::new(read_only_store)
        .load_current()
        .expect("existing local token should load");

    assert_eq!(loaded.generation(), expected.generation());
    assert!(loaded.token().expose_secret() == expected.token().expose_secret());
    assert_root_unchanged(&secret_root, &before);
}

#[test]
fn default_noninteractive_keychain_method_fails_closed_without_calling_interactive_read() {
    let interactive_reads = Arc::new(AtomicUsize::new(0));
    let keychain = InteractiveOnlyKeychain {
        interactive_reads: Arc::clone(&interactive_reads),
    };

    let result = keychain.read_secret_without_user_interaction(
        crate::keychain_data_key::ROUTER_KEYCHAIN_SERVICE,
        "fixture-account",
    );

    assert!(matches!(result, Err(KeychainAccessError::Unavailable)));
    assert_eq!(interactive_reads.load(Ordering::SeqCst), 0);
}

#[test]
fn production_existing_opener_fails_closed_before_constructing_the_keychain_in_tests() {
    let test_root = tempfile::tempdir().expect("isolated parent root");
    let secret_root = test_root.path().join("missing-secrets");

    let result = EncryptedCredentialStore::open_existing_production_for_process(&secret_root);

    assert!(matches!(
        result,
        Err(SecretStoreError::TestKeychainAccessForbidden)
    ));
    assert!(!secret_root.exists());
}

struct InteractiveOnlyKeychain {
    interactive_reads: Arc<AtomicUsize>,
}

impl KeychainAccess for InteractiveOnlyKeychain {
    fn read_secret(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        self.interactive_reads.fetch_add(1, Ordering::SeqCst);
        Ok(Some(vec![0x51; 32]))
    }

    fn add_secret(
        &self,
        _service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        Err(KeychainAccessError::Unavailable)
    }
}
