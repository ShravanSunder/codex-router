//! CLI credential resolver runtime wiring.

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
use thiserror::Error;

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
    secret_store: FileSecretStore,
    now_unix_seconds: u64,
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
            secret_store: FileSecretStore::open(secret_root)?,
            now_unix_seconds,
        })
    }
}

impl ProviderCredentialResolver for CliCredentialResolver {
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
