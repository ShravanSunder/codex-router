use super::*;
use crate::resolver::CredentialRefreshFailure;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::account_tokens::AccountCredentialBundle;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_state::credential_maintenance::CredentialFailureClass;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[derive(Clone)]
struct CountingRefreshClient {
    calls: Arc<AtomicUsize>,
}

impl CredentialRefreshClient for CountingRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(CredentialRefreshFailure::ambiguous(
            CredentialFailureClass::ProviderOutcomeAmbiguous,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        _provider: Provider,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<CredentialBundle, CredentialRefreshFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(CredentialRefreshFailure::ambiguous(
            CredentialFailureClass::ProviderOutcomeAmbiguous,
        ))
    }
}

#[tokio::test]
async fn marking_rejected_generation_does_not_contact_provider() {
    let temp_dir = AuthTestTempDir::new("mark-generation-reauth-without-provider");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let account_id = account_id("mark-generation-reauth-without-provider");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "mark reauth",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let refresh_calls = Arc::new(AtomicUsize::new(0));
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        CountingRefreshClient {
            calls: refresh_calls.clone(),
        },
        Some(1_000),
    );

    assert!(must_ok(
        resolver
            .mark_generation_reauth_required(&account_id, 1)
            .await
    ));
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 0);
}
