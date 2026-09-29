use super::*;

use crate::credential_activation::CredentialActivation;
use crate::credential_activation::CredentialActivationRequest;
use codex_router_core::provider::Provider;

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
