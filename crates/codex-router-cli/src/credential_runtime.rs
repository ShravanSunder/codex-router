//! CLI credential resolver runtime wiring.

use std::path::Path;
use std::path::PathBuf;

use codex_router_auth::resolver::AsyncRefreshLeaseRegistry;
use codex_router_auth::resolver::AsyncRouterCredentialResolver;
use codex_router_auth::resolver::CredentialRefreshClient;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::ProviderCredentialRefreshClients;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
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
    /// Async secret-store construction task failed.
    #[error("credential secret-store construction task failed: {0}")]
    SecretStoreTask(#[from] tokio::task::JoinError),
}

/// CLI-owned credential resolver adapter.
#[derive(Debug)]
pub struct CliCredentialResolver<C = ProviderCredentialRefreshClients>
where
    C: CredentialRefreshClient + Clone,
{
    runtime: tokio::runtime::Runtime,
    state_db_path: PathBuf,
    state_store: AsyncSqliteStateStore,
    secret_store: CliRuntimeSecretStore,
    refresh_client: C,
    refresh_leases: AsyncRefreshLeaseRegistry,
}

/// Async credential resolution used by native-async quota commands.
pub(crate) trait AsyncProviderCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError>;

    async fn recover_unauthorized_credentials_async(
        &self,
        _account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
        _rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        Err(CredentialResolverError::RefreshUnavailable)
    }
}

impl CliCredentialResolver<ProviderCredentialRefreshClients> {
    /// Opens resolver state while reusing the serving process's encrypted-store handle.
    pub(crate) fn open_with_secret_store(
        state_db_path: &Path,
        secret_store: CliRuntimeSecretStore,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let state_store = runtime.block_on(AsyncSqliteStateStore::open(state_db_path))?;
        Ok(Self {
            runtime,
            state_db_path: state_db_path.to_path_buf(),
            state_store,
            secret_store,
            refresh_client: ProviderCredentialRefreshClients::new(),
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
        refresh_client: C,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let state_store = runtime.block_on(AsyncSqliteStateStore::open(state_db_path))?;
        let secret_store =
            codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)?;
        Ok(Self {
            runtime,
            state_db_path: state_db_path.to_path_buf(),
            state_store,
            secret_store: secret_store.into(),
            refresh_client,
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
        })
    }
}

impl<C> ProviderCredentialResolver for CliCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            self.state_store.clone(),
            self.secret_store.clone(),
            self.refresh_client.clone(),
            None,
            self.refresh_leases.clone(),
        );
        self.runtime
            .block_on(resolver.resolve_provider_credentials(account_id, expected_provider))
    }
}

/// CLI credential resolver owned by the process Tokio runtime.
#[derive(Debug)]
pub(crate) struct AsyncCliCredentialResolver<C = ProviderCredentialRefreshClients>
where
    C: CredentialRefreshClient + Clone,
{
    state_store: AsyncSqliteStateStore,
    secret_store: CliRuntimeSecretStore,
    refresh_client: C,
    refresh_leases: AsyncRefreshLeaseRegistry,
}

impl AsyncCliCredentialResolver<ProviderCredentialRefreshClients> {
    /// Opens credential resolver dependencies without creating a nested runtime.
    pub(crate) async fn open(
        state_db_path: &Path,
        secret_root: &Path,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        let state_store = AsyncSqliteStateStore::open(state_db_path).await?;
        let secret_root = secret_root.to_path_buf();
        let secret_store =
            tokio::task::spawn_blocking(move || open_cli_secret_store(&secret_root)).await??;
        Ok(Self {
            state_store,
            secret_store,
            refresh_client: ProviderCredentialRefreshClients::new(),
            refresh_leases: AsyncRefreshLeaseRegistry::new(),
        })
    }
}

impl<C> AsyncProviderCredentialResolver for AsyncCliCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            self.state_store.clone(),
            self.secret_store.clone(),
            self.refresh_client.clone(),
            None,
            self.refresh_leases.clone(),
        );
        resolver
            .resolve_provider_credentials(account_id, expected_provider)
            .await
    }

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            self.state_store.clone(),
            self.secret_store.clone(),
            self.refresh_client.clone(),
            None,
            self.refresh_leases.clone(),
        );
        resolver
            .recover_unauthorized_credentials(account_id, expected_provider, rejected_generation)
            .await
    }
}

impl<C> AsyncProviderCredentialResolver for CliCredentialResolver<C>
where
    C: CredentialRefreshClient + Clone + Send + Sync + 'static,
{
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        let state_store = AsyncSqliteStateStore::open(&self.state_db_path)
            .await
            .map_err(|_error| CredentialResolverError::AccountUnavailable)?;
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            state_store,
            self.secret_store.clone(),
            self.refresh_client.clone(),
            None,
            self.refresh_leases.clone(),
        );
        resolver
            .resolve_provider_credentials(account_id, expected_provider)
            .await
    }

    async fn recover_unauthorized_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: codex_router_core::provider::Provider,
        rejected_generation: u64,
    ) -> Result<(ResolvedProviderCredential, bool), CredentialResolverError> {
        let state_store = AsyncSqliteStateStore::open(&self.state_db_path)
            .await
            .map_err(|_| CredentialResolverError::AccountUnavailable)?;
        let resolver = AsyncRouterCredentialResolver::new_with_refresh_leases(
            state_store,
            self.secret_store.clone(),
            self.refresh_client.clone(),
            None,
            self.refresh_leases.clone(),
        );
        resolver
            .recover_unauthorized_credentials(account_id, expected_provider, rejected_generation)
            .await
    }
}
