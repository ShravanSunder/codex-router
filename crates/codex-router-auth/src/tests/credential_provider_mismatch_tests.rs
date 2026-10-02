use super::*;
use codex_router_core::provider::Provider;

#[tokio::test]
async fn expected_provider_mismatch_prevents_secret_reads_and_maintenance_writes() {
    let temp_dir = AuthTestTempDir::new("expected-provider-mismatch");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("expected-provider-mismatch");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "Claude account",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        NoopCredentialRefreshClient,
        Some(1_000),
    );

    assert!(matches!(
        resolver
            .resolve_provider_credentials(&account_id, Provider::Openai)
            .await,
        Err(CredentialResolverError::AccountProviderMismatch)
    ));
    assert_eq!(
        must_ok(state.load_credential_maintenance(&account_id).await),
        None,
        "provider mismatch must fail before writing credential maintenance"
    );
}
