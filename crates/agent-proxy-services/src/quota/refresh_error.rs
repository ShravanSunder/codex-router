use crate::credential_runtime::CliCredentialResolverOpenError;
use codex_router_auth::resolver::CredentialResolverError;
use codex_router_state::sqlite::StateStoreError;
#[derive(Debug, thiserror::Error)]
pub enum QuotaRefreshError {
    #[error("quota refresh request failed: {message}")]
    ProviderRequest { message: String },
    #[error("quota refresh provider returned HTTP {status}")]
    ProviderStatus { status: u16 },
    #[error("quota refresh provider response was unusable: {message}")]
    ProviderResponse { message: String },
    #[error(transparent)]
    CredentialResolverOpen(#[from] CliCredentialResolverOpenError),
    #[error(transparent)]
    CredentialResolver(#[from] CredentialResolverError),
    #[error(transparent)]
    StateStore(#[from] StateStoreError),
    #[error("failed to initialize quota history runtime: {0}")]
    BackgroundWorkerInitialization(std::io::Error),
    #[error("failed to write stdout: {0}")]
    Stdout(std::io::Error),
}
