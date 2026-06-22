//! Runtime credential resolver factory for the loopback proxy.

use std::path::Path;

use codex_router_auth::resolver::CredentialResolverError;
use codex_router_auth::resolver::NoopCredentialRefreshClient;
use codex_router_auth::resolver::ProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_auth::resolver::RouterCredentialResolver;
use codex_router_core::ids::AccountId;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::SecretStoreError;
use codex_router_state::sqlite::SqliteStateStore;
use codex_router_state::sqlite::StateStoreError;

/// Credential resolver used by proxy runtime entrypoints.
#[derive(Debug)]
pub(crate) struct ProxyCredentialResolver {
    state_store: SqliteStateStore,
    secret_store: FileSecretStore,
    now_unix_seconds: u64,
}

impl ProxyCredentialResolver {
    /// Opens router-owned credential state for runtime resolution.
    pub(crate) fn open(
        state_database_path: &Path,
        secret_store_root: &Path,
        now_unix_seconds: u64,
    ) -> Result<Self, ProxyCredentialResolverOpenError> {
        Ok(Self {
            state_store: SqliteStateStore::open(state_database_path)?,
            secret_store: FileSecretStore::open(secret_store_root)?,
            now_unix_seconds,
        })
    }
}

impl ProviderCredentialResolver for ProxyCredentialResolver {
    fn resolve_provider_credentials(
        &self,
        account_id: &AccountId,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        RouterCredentialResolver::new(
            &self.state_store,
            &self.secret_store,
            NoopCredentialRefreshClient,
            self.now_unix_seconds,
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
