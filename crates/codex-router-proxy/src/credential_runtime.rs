//! Runtime credential resolver factory for the loopback proxy.

use std::path::Path;

use codex_router_auth::resolver::AsyncRefreshLeaseRegistry;
#[cfg(test)]
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::DefaultAsyncRouterCredentialResolver;
#[cfg(test)]
use codex_router_auth::resolver::OpenAiOAuthRefreshClient;
#[cfg(test)]
use codex_router_auth::resolver::ProviderCredentialResolver;
#[cfg(test)]
use codex_router_auth::resolver::RefreshLeaseRegistry;
use codex_router_auth::resolver::ResolvedProviderCredential;
#[cfg(test)]
use codex_router_auth::resolver::RouterCredentialResolver;
#[cfg(test)]
use codex_router_auth::resolver::current_unix_seconds;
use codex_router_core::affinity::RouterAffinityHashSecret;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::affinity_secret::load_or_create_router_affinity_hash_secret;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::sqlite::AsyncSqliteStateStore;
#[cfg(test)]
use codex_router_state::sqlite::SqliteStateStore;
#[cfg(test)]
use codex_router_state::sqlite::StateStoreError;
use futures_util::future::BoxFuture;

use crate::http_sse::AsyncProviderCredentialResolver;
use crate::http_sse::HttpAffinitySecretProvider;
use crate::http_sse::HttpProxyError;
use crate::secret_store_factory::ProxyRuntimeSecretStore;
use crate::secret_store_factory::open_proxy_secret_store;

/// Credential resolver used by proxy runtime entrypoints.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct ProxyCredentialResolver<C = OpenAiOAuthRefreshClient>
where
    C: CredentialRefreshClient + Clone,
{
    state_store: SqliteStateStore,
    secret_store: ProxyRuntimeSecretStore,
    fallback_now_unix_seconds: u64,
    refresh_client: C,
    refresh_leases: RefreshLeaseRegistry,
}

#[cfg(test)]
impl<C> ProxyCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone,
{
    pub(crate) fn open_with_refresh_client(
        state_database_path: &Path,
        secret_store_root: &Path,
        now_unix_seconds: u64,
        refresh_client: C,
    ) -> Result<Self, ProxyCredentialResolverOpenError> {
        Ok(Self {
            state_store: SqliteStateStore::open(state_database_path)?,
            secret_store: open_proxy_secret_store(secret_store_root)?,
            fallback_now_unix_seconds: now_unix_seconds,
            refresh_client,
            refresh_leases: RefreshLeaseRegistry::new(),
        })
    }
}

#[cfg(test)]
impl<C> ProviderCredentialResolver for ProxyCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone,
{
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        RouterCredentialResolver::new_with_refresh_leases(
            &self.state_store,
            &self.secret_store,
            self.refresh_client.clone(),
            current_unix_seconds().unwrap_or(self.fallback_now_unix_seconds),
            self.refresh_leases.clone(),
        )
        .resolve_provider_credentials(account_id)
    }
}

/// Failure opening runtime credential state.
#[cfg(test)]
#[derive(Debug, thiserror::Error)]
pub enum ProxyCredentialResolverOpenError {
    /// State store failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Secret store failed.
    #[error(transparent)]
    Secret(#[from] SecretStoreError),
}

/// Runtime credential and affinity providers opened outside request handling.
#[derive(Clone, Debug)]
pub(crate) struct ProxyRuntimeCredentialResources {
    credential_factory: AsyncProxyCredentialResolverFactory,
    affinity_secret_provider: RuntimeAffinitySecretProvider,
}

impl ProxyRuntimeCredentialResources {
    pub(crate) fn open(
        secret_store_root: &Path,
        fixed_now_unix_seconds: Option<u64>,
    ) -> Result<Self, ProxyRuntimeCredentialResourcesOpenError> {
        let secret_store = open_proxy_secret_store(secret_store_root)?;
        let affinity_secret = load_or_create_router_affinity_hash_secret(&secret_store)
            .map(|loaded| loaded.secret().clone())?;

        Ok(Self {
            credential_factory: AsyncProxyCredentialResolverFactory::new(
                secret_store,
                fixed_now_unix_seconds,
            ),
            affinity_secret_provider: RuntimeAffinitySecretProvider::new(affinity_secret),
        })
    }

    pub(crate) fn credential_factory(&self) -> AsyncProxyCredentialResolverFactory {
        self.credential_factory.clone()
    }

    pub(crate) fn affinity_secret_provider(&self) -> RuntimeAffinitySecretProvider {
        self.affinity_secret_provider.clone()
    }
}

/// Failure opening runtime credential resources.
#[derive(Debug, thiserror::Error)]
pub enum ProxyRuntimeCredentialResourcesOpenError {
    /// Secret store failed.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
}

/// Creates request-scoped async credential resolvers without exposing secret-store internals.
#[derive(Clone, Debug)]
pub(crate) struct AsyncProxyCredentialResolverFactory {
    secret_store: ProxyRuntimeSecretStore,
    refresh_leases: AsyncRefreshLeaseRegistry,
    fixed_now_unix_seconds: Option<u64>,
}

impl AsyncProxyCredentialResolverFactory {
    fn new(secret_store: ProxyRuntimeSecretStore, fixed_now_unix_seconds: Option<u64>) -> Self {
        Self {
            secret_store,
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
            fixed_now_unix_seconds,
        }
    }

    pub(crate) fn resolver_for_state(
        &self,
        state_store: AsyncSqliteStateStore,
    ) -> AsyncProxyCredentialResolver {
        AsyncProxyCredentialResolver::new_default_oauth_with_refresh_leases(
            state_store,
            self.secret_store.clone(),
            self.fixed_now_unix_seconds,
            self.refresh_leases.clone(),
        )
    }
}

/// Async credential resolver used by release `serve` request paths.
pub(crate) type AsyncProxyCredentialResolver =
    DefaultAsyncRouterCredentialResolver<ProxyRuntimeSecretStore>;

impl AsyncProviderCredentialResolver for AsyncProxyCredentialResolver {
    fn resolve_provider_credentials<'a>(
        &'a self,
        account_id: &'a AccountId,
    ) -> BoxFuture<'a, Result<ResolvedProviderCredential, CredentialResolverError>> {
        Box::pin(async move { self.resolve_provider_credentials(account_id).await })
    }
}

/// Preloaded affinity secret provider for request-time HTTP/WS affinity extraction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeAffinitySecretProvider {
    secret: RouterAffinityHashSecret,
}

impl RuntimeAffinitySecretProvider {
    const fn new(secret: RouterAffinityHashSecret) -> Self {
        Self { secret }
    }
}

impl HttpAffinitySecretProvider for RuntimeAffinitySecretProvider {
    fn load_or_create_affinity_secret(&self) -> Result<RouterAffinityHashSecret, HttpProxyError> {
        Ok(self.secret.clone())
    }
}
