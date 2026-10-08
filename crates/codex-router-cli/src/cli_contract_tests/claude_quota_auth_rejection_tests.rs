use super::*;

use codex_router_core::provider::Provider;
use codex_router_core::route_profile::WindowKind;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_state::quota_snapshot::QuotaRefreshErrorClass;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::prelude::*;

struct PersistingRecoveringClaudeCredentialResolver {
    state_path: PathBuf,
    secrets: EncryptedCredentialStore,
}

impl AsyncProviderCredentialResolver for PersistingRecoveringClaudeCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if expected_provider != Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("claude-old-access-canary"),
            1,
        ))
    }

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        if expected_provider != Provider::Claude || rejected_generation != 1 {
            return Err(CredentialResolverError::AccountIneligible);
        }
        let state = AsyncSqliteStateStore::open(&self.state_path)
            .await
            .map_err(|_| CredentialResolverError::RefreshUnavailable)?;
        let account = state
            .load_account(account_id)
            .await
            .map_err(|_| CredentialResolverError::RefreshUnavailable)?
            .ok_or(CredentialResolverError::AccountUnavailable)?;
        state
            .upsert_account(&account.with_active_credential_generation(2))
            .await
            .map_err(|_| CredentialResolverError::RefreshUnavailable)?;
        state
            .close()
            .await
            .map_err(|_| CredentialResolverError::RefreshUnavailable)?;

        let replacement_bundle = CredentialBundle::new_claude(
            SecretString::new("claude-recovered-access-canary"),
            SecretString::new("claude-upkeep-refresh-canary"),
            10_000_000,
        )
        .map_err(|_| CredentialResolverError::RefreshUnavailable)?;
        let replacement_key =
            codex_router_secret_store::account_tokens::provider_credential_bundle_key(
                Provider::Claude,
                account_id,
                2,
            )
            .map_err(|_| CredentialResolverError::SecretUnavailable)?;
        let serialized_bundle = replacement_bundle
            .to_secret_string()
            .map_err(|_| CredentialResolverError::SecretUnavailable)?;
        self.secrets
            .write_secret(&replacement_key, &serialized_bundle)
            .map_err(|_| CredentialResolverError::SecretUnavailable)?;

        Ok((
            ResolvedProviderCredential::new(
                account_id.clone(),
                SecretString::new("claude-recovered-access-canary"),
                2,
            ),
            true,
        ))
    }
}

struct AlwaysUnauthorizedClaudeQuotaProvider {
    access_tokens: Mutex<Vec<String>>,
}

impl QuotaRefreshProvider for AlwaysUnauthorizedClaudeQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        self.access_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.access_token().expose_secret().to_owned());
        Err(crate::quota::QuotaCommandError::ProviderStatus { status: 401 })
    }
}

#[derive(Clone)]
struct RecordingClaudeUpkeepRefreshClient {
    observed_account_ids: tokio::sync::mpsc::UnboundedSender<AccountId>,
}

impl CredentialRefreshClient for RecordingClaudeUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        Err(codex_router_auth::resolver::CredentialRefreshFailure::confirmed_unspent(
            codex_router_state::credential_maintenance::CredentialFailureClass::LocalPersistence,
            None,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        provider: Provider,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<CredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure> {
        assert_eq!(provider, Provider::Claude);
        assert_eq!(
            refresh_token.expose_secret(),
            "claude-upkeep-refresh-canary"
        );
        self.observed_account_ids
            .send(account_id.clone())
            .expect("credential upkeep should observe the account");
        CredentialBundle::new_claude(
            SecretString::new("claude-next-access-canary"),
            SecretString::new("claude-next-refresh-canary"),
            10_000_000,
        )
        .map_err(|_| {
            codex_router_auth::resolver::CredentialRefreshFailure::ambiguous(
                codex_router_state::credential_maintenance::CredentialFailureClass::MalformedResponse,
            )
        })
    }
}

#[derive(Clone, Default)]
struct StructuredEventCapture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

impl StructuredEventCapture {
    fn snapshot(&self) -> Vec<BTreeMap<String, String>> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl<S> Layer<S> for StructuredEventCapture
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _context: Context<'_, S>) {
        let mut visitor = StructuredFieldVisitor::default();
        event.record(&mut visitor);
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(visitor.0);
    }
}

#[derive(Default)]
struct StructuredFieldVisitor(BTreeMap<String, String>);

impl Visit for StructuredFieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }
}

#[test]
fn claude_usage_401_after_renewal_preserves_account_for_upkeep_and_records_failure() {
    let test_root = TestRoot::new("claude-usage-401-after-renewal");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    let account_id = account_id("acct_claude_usage_401");
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(
        runtime.block_on(
            state.upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude usage 401",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            ),
        ),
    );
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let credential_key = must_ok(
        codex_router_secret_store::account_tokens::provider_credential_bundle_key(
            Provider::Claude,
            &account_id,
            1,
        ),
    );
    let stored_bundle = must_ok(CredentialBundle::new_claude(
        SecretString::new("claude-stored-access-canary"),
        SecretString::new("claude-upkeep-refresh-canary"),
        10_000_000,
    ));
    must_ok(secrets.write_secret(&credential_key, &must_ok(stored_bundle.to_secret_string())));
    let existing_window = must_ok(WindowObservation::new(
        WindowObservationProps::new(account_id.clone(), WindowKind::FiveHour, 6_000, 1_500)
            .with_reset_unix_seconds(20_000)
            .with_fresh_until_unix_seconds(2_100),
    ));
    must_ok(runtime.block_on(state.record_window_observation(&existing_window, || 2_000)));
    must_ok(runtime.block_on(state.close()));

    let resolver = PersistingRecoveringClaudeCredentialResolver {
        state_path: state_path.clone(),
        secrets: secrets.clone(),
    };
    let quota_provider = AlwaysUnauthorizedClaudeQuotaProvider {
        access_tokens: Mutex::new(Vec::new()),
    };
    let captured = StructuredEventCapture::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let mut stdout = Vec::new();
    let refresh_result = tracing::subscriber::with_default(subscriber, || {
        runtime.block_on(refresh_quota_store_paths_with_dependencies_async(
            &mut stdout,
            &state_path,
            &secret_root,
            "unused-base-url".to_owned(),
            &resolver,
            &quota_provider,
            2_000,
        ))
    });
    assert!(
        refresh_result.is_err(),
        "the quota refresh should report the repeated provider 401"
    );

    assert_eq!(
        *quota_provider
            .access_tokens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![
            "claude-old-access-canary".to_owned(),
            "claude-recovered-access-canary".to_owned(),
        ]
    );
    let read_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    let account = must_ok(runtime.block_on(read_state.load_account(&account_id)))
        .expect("renewed account should remain stored");
    assert_eq!(account.status(), AccountStatus::Enabled);
    assert_eq!(account.active_credential_generation(), Some(2));
    let refresh_status = must_ok(
        runtime.block_on(read_state.quota_refresh_statuses_for_route_band("claude_messages")),
    )
    .into_iter()
    .find(|status| status.account_id() == &account_id)
    .expect("Claude quota refresh failure should be recorded");
    assert_eq!(
        refresh_status.last_error_class(),
        Some(QuotaRefreshErrorClass::AuthError)
    );
    let windows =
        must_ok(runtime.block_on(read_state.window_observations_for_account(&account_id)));
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].remaining_basis_points(), 6_000);

    let captured_events = captured.snapshot();
    let renewed_credential_rejection = captured_events
        .iter()
        .find(|event| {
            event.get("message").is_some_and(|message| {
                message.contains("Claude usage endpoint rejected a freshly renewed credential")
            })
        })
        .expect("renewed-credential 401 event should be captured");
    let account_hash = renewed_credential_rejection
        .get("account.hash")
        .expect("account hash field should be captured");
    assert!(!account_hash.is_empty());
    assert_ne!(account_hash, account_id.as_str());
    assert_eq!(
        renewed_credential_rejection
            .get("credential_generation")
            .map(String::as_str),
        Some("2")
    );
    assert_eq!(
        renewed_credential_rejection
            .get("http.status_code")
            .map(String::as_str),
        Some("401")
    );
    assert_eq!(
        renewed_credential_rejection
            .get("endpoint.path")
            .map(String::as_str),
        Some("/api/oauth/usage")
    );
    let captured_fields = format!("{captured_events:?}");
    assert!(!captured_fields.contains(account_id.as_str()));
    assert!(!captured_fields.contains("claude-old-access-canary"));
    assert!(!captured_fields.contains("claude-recovered-access-canary"));
    must_ok(runtime.block_on(read_state.close()));

    let (observed_sender, mut observed_receiver) = tokio::sync::mpsc::unbounded_channel();
    runtime.block_on(async {
        let mut upkeep_worker = must_ok(
            crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock(
                state_path.clone(),
                secrets,
                RecordingClaudeUpkeepRefreshClient {
                    observed_account_ids: observed_sender,
                },
                || 2_000,
            )
            .await,
        );
        let observed_account = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            observed_receiver.recv(),
        )
        .await
        .unwrap_or_else(|error| panic!("upkeep should observe the renewed account: {error}"))
        .expect("upkeep refresh should report its account");
        upkeep_worker.shutdown().await;
        assert_eq!(observed_account, account_id);
    });
}
