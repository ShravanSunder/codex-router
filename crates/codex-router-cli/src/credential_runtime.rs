//! CLI credential resolver runtime wiring.

use std::path::Path;

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
}

/// CLI-owned credential resolver adapter.
#[derive(Debug)]
pub struct CliCredentialResolver {
    state_store: SqliteStateStore,
    secret_store: CliRuntimeSecretStore,
    fallback_now_unix_seconds: u64,
    refresh_client: OpenAiOAuthRefreshClient,
    refresh_leases: RefreshLeaseRegistry,
}

impl CliCredentialResolver {
    /// Opens CLI credential resolver dependencies.
    pub fn open(
        state_db_path: &Path,
        secret_root: &Path,
        now_unix_seconds: u64,
    ) -> Result<Self, CliCredentialResolverOpenError> {
        Ok(Self {
            state_store: SqliteStateStore::open(state_db_path)?,
            secret_store: open_cli_secret_store(secret_root)?,
            fallback_now_unix_seconds: now_unix_seconds,
            refresh_client: OpenAiOAuthRefreshClient::new(),
            refresh_leases: RefreshLeaseRegistry::new(),
        })
    }
}

impl ProviderCredentialResolver for CliCredentialResolver {
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
