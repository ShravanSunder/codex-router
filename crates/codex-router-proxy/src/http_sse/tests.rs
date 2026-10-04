use super::AsyncClaudeQuotaObservationWriter;
use super::AsyncProviderCredentialResolver;
use super::AuthenticatedHttpProxyService;
use super::HttpAffinitySecretProvider;
use super::HttpProxyError;
use super::HttpProxyRequest;
use super::audit_route_kind_for_route_kind;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::audit::RouteKind as AuditRouteKind;
use codex_router_core::ids::AccountId;
use codex_router_core::ids::TokenGeneration;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::CLAUDE_MESSAGES;
use codex_router_core::route_profile::WindowKind;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::calculate_window_observation_fresh_until_unix_seconds;
use futures_util::future::BoxFuture;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use crate::account_selection::AsyncAccountDecisionSelector;
use crate::account_selection::SelectedAccountDecision;
use crate::headers::Header;
use crate::headers::HeaderCollection;
use crate::local_auth::ProxyLocalAuthGate;
use crate::routes::Method;
use crate::routes::RouteKind;

#[test]
fn image_routes_map_to_distinct_audit_route_kinds() {
    assert_eq!(
        audit_route_kind_for_route_kind(RouteKind::ImageGenerations),
        AuditRouteKind::ImageGenerations
    );
    assert_eq!(
        audit_route_kind_for_route_kind(RouteKind::ImageEdits),
        AuditRouteKind::ImageEdits
    );
}

#[test]
fn claude_messages_route_maps_to_its_audit_route_kind() {
    assert_eq!(
        audit_route_kind_for_route_kind(RouteKind::ClaudeMessages),
        AuditRouteKind::ClaudeMessages
    );
}

struct PassiveObservationTempDir {
    path: PathBuf,
}

impl PassiveObservationTempDir {
    fn new() -> Self {
        static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "codex-router-proxy-passive-observation-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap_or_else(|error| {
            panic!("passive observation test directory should be created: {error}")
        });
        Self { path }
    }

    fn database_path(&self) -> PathBuf {
        self.path.join("state.sqlite")
    }
}

impl Drop for PassiveObservationTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct SqlitePassiveObservationWriter {
    state: Arc<AsyncSqliteStateStore>,
    application_unix_seconds: u64,
}

impl AsyncClaudeQuotaObservationWriter for SqlitePassiveObservationWriter {
    fn record_window_observation<'a>(
        &'a self,
        observation: WindowObservation,
    ) -> BoxFuture<'a, Result<(), StateStoreError>> {
        let state = Arc::clone(&self.state);
        let application_unix_seconds = self.application_unix_seconds;
        Box::pin(async move {
            state
                .record_window_observation(&observation, || application_unix_seconds)
                .await
                .map(|_observation_was_newest| ())
        })
    }
}

#[derive(Clone)]
struct FixedAsyncSelector {
    account_id: AccountId,
}

impl AsyncAccountDecisionSelector for FixedAsyncSelector {
    fn select_upstream_account<'a>(
        &'a self,
        _request: &'a HttpProxyRequest,
        _token_generation: TokenGeneration,
        _affinity_secret: Option<&'a codex_router_core::affinity::RouterAffinityHashSecret>,
    ) -> BoxFuture<'a, Result<SelectedAccountDecision, HttpProxyError>> {
        let account_id = self.account_id.clone();
        Box::pin(async move { Ok(SelectedAccountDecision::new(account_id, "test-fixed")) })
    }
}

struct FixedAffinitySecretProvider;

impl HttpAffinitySecretProvider for FixedAffinitySecretProvider {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        Ok(RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap_or_else(|error| panic!("test affinity secret should be valid: {error}")))
    }
}

#[derive(Clone)]
struct ProviderCheckingAsyncCredentialResolver {
    account_id: AccountId,
    accepted_provider: Provider,
    requested_providers: Arc<Mutex<Vec<Provider>>>,
}

impl AsyncProviderCredentialResolver for ProviderCheckingAsyncCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        _account_id: &'a AccountId,
        expected_provider: Provider,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        let account_id = self.account_id.clone();
        let accepted_provider = self.accepted_provider;
        let requested_providers = Arc::clone(&self.requested_providers);
        Box::pin(async move {
            requested_providers
                .lock()
                .unwrap_or_else(|_error| panic!("provider request lock should be available"))
                .push(expected_provider);
            if expected_provider != accepted_provider {
                return Err(CredentialResolverError::AccountProviderMismatch);
            }
            Ok(ResolvedProviderCredential::new(
                account_id,
                SecretString::new("test-access-token"),
                1,
            ))
        })
    }
}

#[tokio::test]
async fn credential_resolution_uses_each_route_profile_provider() {
    for (path, expected_provider) in [
        ("/anthropic/v1/messages", Provider::Claude),
        ("/v1/responses", Provider::Openai),
    ] {
        let account_id = AccountId::new("acct_provider_test")
            .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
        let requested_providers = Arc::new(Mutex::new(Vec::new()));
        let resolver = ProviderCheckingAsyncCredentialResolver {
            account_id: account_id.clone(),
            accepted_provider: expected_provider,
            requested_providers: Arc::clone(&requested_providers),
        };
        let selector = FixedAsyncSelector { account_id };
        let auth_gate = ProxyLocalAuthGate::disabled();
        let upstream = ();
        let affinity_secret_provider = FixedAffinitySecretProvider;
        let service =
            AuthenticatedHttpProxyService::new(&auth_gate, &selector, &resolver, &upstream)
                .with_affinity_secret_provider(&affinity_secret_provider);

        let prepared = service
            .prepare_streaming_request_async(HttpProxyRequest::new(Method::Post, path))
            .await;
        let prepared = prepared.unwrap_or_else(|_error| {
            panic!("route {path} should resolve a {expected_provider:?} credential")
        });
        assert_eq!(
            *requested_providers
                .lock()
                .unwrap_or_else(|_error| panic!("provider request lock should be available")),
            vec![expected_provider],
            "route {path} should pass its profile provider to credential resolution"
        );
        let (_upstream_request, _completion) = prepared.into_parts();
    }
}

#[tokio::test]
async fn passive_claude_headers_persist_active_equivalent_freshness_for_non_default_interval() {
    let temp_dir = PassiveObservationTempDir::new();
    let state = Arc::new(
        AsyncSqliteStateStore::open(&temp_dir.database_path())
            .await
            .unwrap_or_else(|error| panic!("state database should open: {error}")),
    );
    let account_id = AccountId::new("acct_passive_freshness_test")
        .unwrap_or_else(|error| panic!("account id should be valid: {error}"));
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                "passive-freshness-test",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .unwrap_or_else(|error| panic!("Claude account should persist: {error}"));
    let writer = SqlitePassiveObservationWriter {
        state: Arc::clone(&state),
        application_unix_seconds: 100,
    };
    let headers = HeaderCollection::new(vec![
        Header::new("anthropic-ratelimit-unified-5h-utilization", "0.25"),
        Header::new("anthropic-ratelimit-unified-5h-reset", "500"),
        Header::new("anthropic-ratelimit-unified-7d-utilization", "0.8"),
        Header::new("anthropic-ratelimit-unified-7d-reset", "900"),
    ]);
    let request_started_at = 100;
    let refresh_interval = std::time::Duration::from_secs(400);
    super::record_passive_claude_quota_observations(
        &CLAUDE_MESSAGES,
        &account_id,
        &headers,
        request_started_at,
        refresh_interval,
        &writer,
    )
    .await
    .unwrap_or_else(|error| panic!("passive observations should persist: {error}"));
    let active_fresh_until = calculate_window_observation_fresh_until_unix_seconds(
        request_started_at,
        refresh_interval.as_secs(),
    )
    .unwrap_or_else(|error| panic!("active freshness should be valid: {error}"));

    assert_eq!(
        std::time::Duration::from_secs(active_fresh_until - request_started_at),
        std::time::Duration::from_secs(520)
    );
    assert_eq!(active_fresh_until, request_started_at + 520);
    let observations = state
        .window_observations_for_account(&account_id)
        .await
        .unwrap_or_else(|error| panic!("persisted passive observations should load: {error}"));
    assert_eq!(observations.len(), 2);
    for observation in &observations {
        assert_eq!(observation.observation_started_at(), request_started_at);
        assert_eq!(
            observation.fresh_until_unix_seconds(),
            Some(active_fresh_until)
        );
    }
    assert_eq!(
        observations
            .iter()
            .find(|observation| observation.window_kind() == WindowKind::FiveHour)
            .map(|observation| observation.remaining_basis_points()),
        Some(7_500)
    );
    assert_eq!(
        observations
            .iter()
            .find(|observation| observation.window_kind() == WindowKind::Weekly)
            .map(|observation| observation.remaining_basis_points()),
        Some(2_000)
    );
}
