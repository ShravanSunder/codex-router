use super::*;
use std::sync::Mutex;

use crate::credential_activation::CredentialActivation;
use crate::credential_activation::CredentialActivationRequest;
use codex_router_core::provider::Provider;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStoreStatus;
use codex_router_secret_store::keychain_data_key::KeychainAccess;
use codex_router_secret_store::keychain_data_key::KeychainAccessError;
use codex_router_secret_store::keychain_data_key::ROUTER_KEYCHAIN_SERVICE;

#[derive(Default)]
struct LoginTestKeychainAccess(Mutex<Option<Vec<u8>>>);

impl KeychainAccess for LoginTestKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        Ok(self.0.lock().expect("test Keychain lock").clone())
    }

    fn add_secret(
        &self,
        service: &str,
        _account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        let mut stored_key = self.0.lock().expect("test Keychain lock");
        if stored_key.is_some() {
            return Err(KeychainAccessError::Unavailable);
        }
        *stored_key = Some(secret.to_vec());
        Ok(())
    }
}

#[derive(Clone)]
struct FailingStagedCredentialStore(
    codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore,
);

impl codex_router_secret_store::SecretStore for FailingStagedCredentialStore {
    fn write_secret(
        &self,
        key: &codex_router_secret_store::model::SecretKey,
        secret: &codex_router_core::redaction::SecretString,
    ) -> Result<(), codex_router_secret_store::model::SecretStoreError> {
        codex_router_secret_store::SecretStore::write_secret(&self.0, key, secret)
    }

    fn read_secret(
        &self,
        key: &codex_router_secret_store::model::SecretKey,
    ) -> Result<
        codex_router_core::redaction::SecretString,
        codex_router_secret_store::model::SecretStoreError,
    > {
        codex_router_secret_store::SecretStore::read_secret(&self.0, key)
    }

    fn write_staged(
        &self,
        _key: &codex_router_secret_store::model::SecretKey,
        _secret: &codex_router_core::redaction::SecretString,
    ) -> Result<(), codex_router_secret_store::model::SecretStoreError> {
        Err(codex_router_secret_store::model::SecretStoreError::KeyUnavailable)
    }
}

#[test]
fn production_opener_marks_a_fresh_store_ready_for_login() {
    let temp_dir = AuthTestTempDir::new("production-opener-fresh-store");
    let state_path = temp_dir.path().join("state.sqlite");
    let secret_root = temp_dir.path().join("secrets");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let keychain = LoginTestKeychainAccess::default();
    let secrets = must_ok(EncryptedCredentialStore::open_for_process_with_keychain(
        &secret_root,
        &keychain,
    ));
    assert_eq!(secrets.status(), EncryptedCredentialStoreStatus::Ready);
    let account_id = account_id("fresh-production-open-login");
    let request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        "fresh root login",
        AccountCredentialBundle::imported_codex_auth(
            "fresh-root-access-canary",
            Some("fresh-root-refresh-canary".to_owned()),
        ),
    );

    let generation = must_ok(runtime.block_on(CredentialActivation::activate_login(
        &state, &secrets, request,
    )));

    assert_eq!(generation, 1);
    let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let stored = must_ok(secrets.read_secret(&key));
    assert!(stored.expose_secret().contains("fresh-root-access-canary"));
}

#[test]
fn login_activation_claims_first_generation_and_removes_transient_claim() {
    let temp_dir = AuthTestTempDir::new("login-activation-first-generation");
    let state_path = temp_dir.path().join("state.sqlite");
    let secret_root = temp_dir.path().join("secrets");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("login-activation-first-generation");
    let request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        "first login",
        AccountCredentialBundle::imported_codex_auth(
            "first-login-access-canary",
            Some("first-login-refresh-canary".to_owned()),
        ),
    );

    let generation = must_ok(runtime.block_on(CredentialActivation::activate_login(
        &state, &secrets, request,
    )));

    assert_eq!(generation, 1);
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("new login should register its account");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(1));
    assert_eq!(
        must_ok(runtime.block_on(state.load_credential_maintenance(&account_id))),
        None
    );
    let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let stored_bundle = must_ok(AccountCredentialBundle::from_secret_string(must_ok(
        secrets.read_secret(&key),
    )));
    assert_eq!(
        stored_bundle.access_token().expose_secret(),
        "first-login-access-canary"
    );
}

#[test]
fn failed_login_staged_write_restores_healthy_maintenance() {
    let temp_dir = AuthTestTempDir::new("login-write-failure-preserves-health");
    let state_path = temp_dir.path().join("state.sqlite");
    let secret_root = temp_dir.path().join("secrets");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("login-write-failure-preserves-health");
    must_ok(
        runtime.block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    "healthy account",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    assert!(must_ok(runtime.block_on(state.claim_credential_refresh(
        &account_id,
        Provider::Openai,
        codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
        1,
        2,
    ))));
    assert!(must_ok(runtime.block_on(
        state.activate_claimed_credential_generation(
            &account_id,
            Provider::Openai,
            codex_router_state::credential_maintenance::ClaimPurpose::Refresh,
            1,
            2,
            1_000,
        )
    )));
    let maintenance_before =
        must_ok(runtime.block_on(state.load_credential_maintenance(&account_id)))
            .expect("successful refresh creates healthy maintenance");
    let failing_store = FailingStagedCredentialStore(secrets);
    let request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        "healthy account",
        AccountCredentialBundle::imported_codex_auth(
            "failed-login-access-canary",
            Some("failed-login-refresh-canary".to_owned()),
        ),
    );

    let result = runtime.block_on(CredentialActivation::activate_login(
        &state,
        &failing_store,
        request,
    ));
    let maintenance_after =
        must_ok(runtime.block_on(state.load_credential_maintenance(&account_id)))
            .expect("maintenance remains present");

    assert!(matches!(
        result,
        Err(crate::credential_activation::CredentialActivationError::CredentialStoreUnavailable)
    ));
    assert_eq!(maintenance_after, maintenance_before);
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("existing account remains registered");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(2));
}

#[test]
fn login_activation_reenables_a_disabled_account_on_the_next_generation() {
    let temp_dir = AuthTestTempDir::new("login-activation-disabled-account");
    let state_path = temp_dir.path().join("state.sqlite");
    let secret_root = temp_dir.path().join("secrets");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("login-activation-disabled-account");
    must_ok(
        runtime.block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    "disabled login",
                    AccountStatus::Disabled,
                )
                .with_active_credential_generation(4),
            ),
        ),
    );
    let old_key = must_ok(openai_account_credential_bundle_key(&account_id, 4));
    must_ok(
        secrets.write_secret(
            &old_key,
            &AccountCredentialBundle::imported_codex_auth(
                "old-access-canary",
                Some("old-refresh-canary".to_owned()),
            )
            .to_secret_string()
            .expect("old credential should serialize"),
        ),
    );
    let request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        "disabled login",
        AccountCredentialBundle::imported_codex_auth(
            "new-access-canary",
            Some("new-refresh-canary".to_owned()),
        ),
    );

    let generation = must_ok(runtime.block_on(CredentialActivation::activate_login(
        &state, &secrets, request,
    )));

    assert_eq!(generation, 5);
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("disabled account should remain registered");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(5));
    assert_eq!(
        must_ok(runtime.block_on(state.load_credential_maintenance(&account_id))),
        None
    );
}

#[test]
fn login_activation_rejects_a_provider_mismatch_before_writing() {
    let temp_dir = AuthTestTempDir::new("login-activation-provider-mismatch");
    let state_path = temp_dir.path().join("state.sqlite");
    let secret_root = temp_dir.path().join("secrets");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("login-activation-provider-mismatch");
    must_ok(runtime.block_on(state.upsert_account(&AccountRecord::new(
        Provider::Claude,
        account_id.clone(),
        "claude account",
        AccountStatus::Disabled,
    ))));
    let request = CredentialActivationRequest::new(
        Provider::Openai,
        account_id.clone(),
        "claude account",
        AccountCredentialBundle::imported_codex_auth(
            "must-not-write-access-canary",
            Some("must-not-write-refresh-canary".to_owned()),
        ),
    );

    let result = runtime.block_on(CredentialActivation::activate_login(
        &state, &secrets, request,
    ));

    assert!(matches!(
        result,
        Err(crate::credential_activation::CredentialActivationError::AccountProviderMismatch)
    ));
    let openai_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    assert!(secrets.read_secret(&openai_key).is_err());
    let account = must_ok(runtime.block_on(state.load_account(&account_id)))
        .expect("Claude account should remain registered");
    assert_eq!(account.provider(), Provider::Claude);
    assert_eq!(account.active_credential_generation(), None);
}
