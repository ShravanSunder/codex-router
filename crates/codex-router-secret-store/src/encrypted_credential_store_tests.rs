use std::fs;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;

use crate::SecretStore;
use crate::account_tokens::openai_account_credential_bundle_key;
use crate::encrypted_credential_store::EncryptedCredentialStore;
use crate::encrypted_credential_store::EncryptedCredentialStoreStatus;
use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::model::SecretKey;
use crate::model::SecretStoreError;
use crate::model::StoreUnavailable;

#[test]
fn staged_credential_write_keeps_plaintext_out_of_files_and_authenticates_key_name() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let file_store = FileSecretStore::open(root.path()).expect("file secret store");
    let store =
        EncryptedCredentialStore::new(file_store, PooledCredentialDataKey::from_bytes([0x42; 32]));
    let account_id = AccountId::new("acct_encrypted_store").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let other_key =
        crate::account_tokens::provider_credential_bundle_key(Provider::Claude, &account_id, 1)
            .expect("other provider key");
    let plaintext = SecretString::new("access-token-canary-refresh-token-canary");

    store
        .write_staged(&credential_key, &plaintext)
        .expect("encrypted staged write");

    let envelope_path = root.path().join(format!("{}.v2", credential_key.as_str()));
    let plaintext_path = root
        .path()
        .join(format!("{}.secret", credential_key.as_str()));
    let envelope_bytes = fs::read(&envelope_path).expect("versioned envelope");
    let envelope: serde_json::Value =
        serde_json::from_slice(&envelope_bytes).expect("JSON envelope");
    let nonce_text = envelope["nonce"].as_str().expect("nonce text");
    let ciphertext_text = envelope["ciphertext"].as_str().expect("ciphertext text");

    assert_eq!(envelope["format"].as_u64(), Some(2));
    assert_eq!(STANDARD.decode(nonce_text).expect("nonce base64").len(), 12);
    assert!(!String::from_utf8_lossy(&envelope_bytes).contains("access-token-canary"));
    assert!(!String::from_utf8_lossy(&envelope_bytes).contains("refresh-token-canary"));
    assert!(!plaintext_path.exists());
    assert_eq!(
        store
            .read_secret(&credential_key)
            .expect("authenticated read")
            .expose_secret(),
        plaintext.expose_secret()
    );

    let other_envelope_path = root.path().join(format!("{}.v2", other_key.as_str()));
    fs::write(&other_envelope_path, envelope_bytes).expect("copied envelope");
    let decrypt_result = store.read_secret(&other_key);

    assert!(
        decrypt_result.is_err(),
        "AAD must bind ciphertext to its key name"
    );
    assert!(!ciphertext_text.is_empty());
}

#[test]
fn a_different_process_data_key_cannot_decrypt_shared_ciphertext() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let account_id = AccountId::new("acct_wrong_data_key").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let writer = EncryptedCredentialStore::new(
        FileSecretStore::open(root.path()).expect("writer file store"),
        PooledCredentialDataKey::from_bytes([0x42; 32]),
    );
    writer
        .write_staged(&credential_key, &SecretString::new("pooled-token-canary"))
        .expect("credential should be encrypted");
    let unrelated_process = EncryptedCredentialStore::new(
        FileSecretStore::open(root.path()).expect("reader file store"),
        PooledCredentialDataKey::from_bytes([0x43; 32]),
    );

    let result = unrelated_process.read_secret(&credential_key);

    assert!(matches!(
        result,
        Err(SecretStoreError::CredentialAuthenticationFailed)
    ));
}

#[test]
fn key_unavailable_never_falls_back_to_legacy_plaintext() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let file_store = FileSecretStore::open(root.path()).expect("file secret store");
    let account_id = AccountId::new("acct_locked_fallback").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let legacy_path = root
        .path()
        .join(format!("{}.secret", credential_key.as_str()));
    fs::write(&legacy_path, "legacy-token-canary").expect("legacy fixture");
    let store = EncryptedCredentialStore::key_unavailable(file_store);

    let error = store
        .read_secret(&credential_key)
        .expect_err("locked Keychain must not read plaintext");

    assert!(matches!(error, SecretStoreError::KeyUnavailable));
    assert!(!error.to_string().contains("legacy-token-canary"));
}

#[test]
fn incomplete_migration_blocks_credentials_but_keeps_router_secrets_available() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let file_store = FileSecretStore::open(root.path()).expect("file secret store");
    let account_id = AccountId::new("acct_incomplete_migration").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    let local_token_key = SecretKey::new("local_router_token").expect("local token key");
    let local_token = SecretString::new("local-router-token-canary");
    fs::write(
        root.path()
            .join(format!("{}.secret", credential_key.as_str())),
        "legacy-token-canary",
    )
    .expect("legacy credential fixture");
    file_store
        .write_secret(&local_token_key, &local_token)
        .expect("local token stays in file store");
    let store = EncryptedCredentialStore::migration_incomplete(
        file_store,
        vec![account_id.as_str().to_owned()],
        crate::credential_migration::CredentialMigrationFailure::MigrationNotComplete,
    );

    assert_eq!(
        store.status(),
        EncryptedCredentialStoreStatus::MigrationIncomplete {
            accounts: vec![account_id.as_str().to_owned()],
            failure: crate::credential_migration::CredentialMigrationFailure::MigrationNotComplete,
        }
    );
    assert_eq!(
        store
            .read_secret(&local_token_key)
            .expect("local token remains available")
            .expose_secret(),
        local_token.expose_secret()
    );
    assert!(matches!(
        store.read_secret(&credential_key),
        Err(SecretStoreError::StoreUnavailable(
            StoreUnavailable::MigrationIncomplete { accounts, failure }
        )) if accounts == vec![account_id.as_str()]
            && failure == crate::credential_migration::CredentialMigrationFailure::MigrationNotComplete
    ));
}

#[test]
fn ready_store_reads_only_v2_and_never_legacy_credential_files() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let file_store = FileSecretStore::open(root.path()).expect("file secret store");
    let account_id = AccountId::new("acct_v2_only").expect("account id");
    let credential_key =
        openai_account_credential_bundle_key(&account_id, 1).expect("credential key");
    fs::write(
        root.path()
            .join(format!("{}.secret", credential_key.as_str())),
        "legacy-token-canary",
    )
    .expect("legacy credential fixture");
    let store =
        EncryptedCredentialStore::new(file_store, PooledCredentialDataKey::from_bytes([0x27; 32]));

    let error = store
        .read_secret(&credential_key)
        .expect_err("normal reads must use v2 only");

    assert!(matches!(error, SecretStoreError::Filesystem { .. }));
    assert!(!error.to_string().contains("legacy-token-canary"));
}

#[test]
fn generation_pruning_keeps_active_and_previous_and_isolated_by_provider_and_account() {
    let root = tempfile::tempdir().expect("temporary secret root");
    let store = EncryptedCredentialStore::new(
        FileSecretStore::open(root.path()).expect("file secret store"),
        PooledCredentialDataKey::from_bytes([0x62; 32]),
    );
    let account_id = AccountId::new("acct_prune_generations").expect("account id");
    let other_account_id = AccountId::new("acct_prune_other").expect("other account id");
    for (provider, owner, generation) in [
        (Provider::Openai, &account_id, 1),
        (Provider::Openai, &account_id, 2),
        (Provider::Openai, &account_id, 3),
        (Provider::Openai, &account_id, 4),
        (Provider::Claude, &account_id, 1),
        (Provider::Claude, &account_id, 2),
        (Provider::Claude, &other_account_id, 1),
    ] {
        let key =
            crate::account_tokens::provider_credential_bundle_key(provider, owner, generation)
                .expect("provider-scoped key");
        store
            .write_staged(&key, &SecretString::new(format!("opaque-{generation}")))
            .expect("credential generation should be encrypted");
    }

    let removed = store
        .prune_obsolete_generations(Provider::Openai, &account_id, 4)
        .expect("obsolete OpenAI generations should be pruned");

    assert_eq!(removed, vec![1, 2]);
    for (provider, owner, generation, should_remain) in [
        (Provider::Openai, &account_id, 1, false),
        (Provider::Openai, &account_id, 2, false),
        (Provider::Openai, &account_id, 3, true),
        (Provider::Openai, &account_id, 4, true),
        (Provider::Claude, &account_id, 1, true),
        (Provider::Claude, &account_id, 2, true),
        (Provider::Claude, &other_account_id, 1, true),
    ] {
        let key =
            crate::account_tokens::provider_credential_bundle_key(provider, owner, generation)
                .expect("provider-scoped key");
        assert_eq!(store.read_secret(&key).is_ok(), should_remain);
    }
}
