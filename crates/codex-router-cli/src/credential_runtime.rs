//! CLI credential resolver runtime wiring.

use std::path::Path;

use codex_router_auth::resolver::AsyncRefreshLeaseRegistry;
use codex_router_auth::resolver::AsyncRouterCredentialResolver;
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::OpenAiOAuthRefreshClient;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_auth::resolver::current_unix_seconds;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::sqlite::StateStoreError;
use thiserror::Error;

use crate::secret_store_factory::CliRuntimeSecretStore;
use crate::secret_store_factory::open_cli_secret_store;

/// CLI credential resolver open failure.
#[derive(Debug, Error)]
pub enum CliCredentialResolverOpenError {
    /// State database failed to open.
    #[error(transparent)]
    StateStore(#[from] StateStoreError),
    /// Secret store failed to open.
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    /// Tokio runtime failed to initialize.
    #[error(transparent)]
    Runtime(#[from] std::io::Error),
}

/// CLI-owned credential resolver adapter.
#[derive(Debug)]
pub struct CliCredentialResolver<C = OpenAiOAuthRefreshClient>
where
    C: CredentialRefreshClient + Clone,
{
    runtime: tokio::runtime::Runtime,
    state_store: AsyncSqliteStateStore,
    secret_store: CliRuntimeSecretStore,
    fallback_now_unix_seconds: u64,
    refresh_client: C,
    refresh_leases: AsyncRefreshLeaseRegistry,
}

impl CliCredentialResolver<OpenAiOAuthRefreshClient> {
    /// Opens CLI credential resolver dependencies.
    pub fn open(
        state_db_path: &Path,
        secret_root: &Path,
        now_unix_seconds: u64,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let state_store = runtime.block_on(AsyncSqliteStateStore::open(state_db_path))?;
        Ok(Self {
            runtime,
            state_store,
            secret_store: open_cli_secret_store(secret_root)?,
            fallback_now_unix_seconds: now_unix_seconds,
            refresh_client: OpenAiOAuthRefreshClient::new(),
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
        })
    }
}

#[cfg(test)]
impl<C> CliCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone,
{
    pub(crate) fn open_with_refresh_client(
        state_db_path: &Path,
        secret_root: &Path,
        now_unix_seconds: u64,
        refresh_client: C,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let state_store = runtime.block_on(AsyncSqliteStateStore::open(state_db_path))?;
        Ok(Self {
            runtime,
            state_store,
            secret_store: open_cli_secret_store(secret_root)?,
            fallback_now_unix_seconds: now_unix_seconds,
            refresh_client,
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
        })
    }
}

impl<C> ProviderCredentialResolver for CliCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone + Send + 'static,
{
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            self.state_store.clone(),
            self.secret_store.clone(),
            self.refresh_client.clone(),
            Some(current_unix_seconds().unwrap_or(self.fallback_now_unix_seconds)),
            self.refresh_leases.clone(),
        );
        self.runtime
            .block_on(resolver.resolve_provider_credentials(account_id))
    }
}
