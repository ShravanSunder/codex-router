//! Runtime credential resolver factory for the loopback proxy.

use std::path::Path;

use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::OpenAiOAuthRefreshClient;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::RefreshLeaseRegistry;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_auth::resolver::RouterCredentialResolver;
use codex_router_auth::resolver::current_unix_seconds;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

use crate::secret_store_factory::ProxyRuntimeSecretStore;
use crate::secret_store_factory::open_proxy_secret_store;

/// Credential resolver used by proxy runtime entrypoints.
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

impl ProxyCredentialResolver<OpenAiOAuthRefreshClient> {
    /// Opens router-owned credential state for runtime resolution.
    pub(crate) fn open(
        state_database_path: &Path,
        secret_store_root: &Path,
        now_unix_seconds: u64,
    ) -> Result<Self, ProxyCredentialResolverOpenError> {
        Self::open_with_refresh_client(
            state_database_path,
            secret_store_root,
            now_unix_seconds,
            OpenAiOAuthRefreshClient::new(),
        )
    }
}

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
#[derive(Debug, thiserror::Error)]
pub enum ProxyCredentialResolverOpenError {
    /// State store failed.
    #[error(transparent)]
    State(#[from] StateStoreError),
    /// Secret store failed.
    #[error(transparent)]
    Secret(#[from] SecretStoreError),
}
