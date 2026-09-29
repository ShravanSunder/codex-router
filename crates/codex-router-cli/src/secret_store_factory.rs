//! CLI runtime secret-store factory.

use std::io;
use std::path::Path;
use std::path::PathBuf;

use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::model::SecretStoreError;
use thiserror::Error;

/// CLI runtime secret-store backend.
pub(crate) type CliRuntimeSecretStore = EncryptedCredentialStore;

/// Opens the CLI runtime secret store.
pub(crate) fn open_cli_secret_store(
    secret_root: &Path,
) -> Result<CliRuntimeSecretStore, SecretStoreError> {
    #[cfg(any(test, feature = "keychain-test-support"))]
    {
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
    }
    #[cfg(not(any(test, feature = "keychain-test-support")))]
    {
        EncryptedCredentialStore::open_production_for_process(secret_root)
    }
}

/// Opens one CLI process handle from an async Tokio context.
pub(crate) async fn open_cli_secret_store_async(
    secret_root: impl Into<PathBuf>,
) -> Result<CliRuntimeSecretStore, CliSecretStoreOpenError> {
    let secret_root = secret_root.into();
    tokio::task::spawn_blocking(move || open_cli_secret_store(&secret_root))
        .await
        .map_err(CliSecretStoreOpenError::Task)?
        .map_err(CliSecretStoreOpenError::SecretStore)
}

/// Blocking secret-store construction failures at the CLI edge.
#[derive(Debug, Error)]
pub(crate) enum CliSecretStoreOpenError {
    #[error("credential store blocking runtime failed")]
    Runtime(#[source] io::Error),
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    #[error("credential store construction task failed")]
    Task(#[source] tokio::task::JoinError),
}

/// Opens one process handle with Keychain and filesystem work on Tokio's blocking pool.
pub(crate) fn open_cli_secret_store_in_spawn_blocking(
    secret_root: impl Into<PathBuf>,
) -> Result<CliRuntimeSecretStore, CliSecretStoreOpenError> {
    let secret_root = secret_root.into();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(CliSecretStoreOpenError::Runtime)?;
    runtime.block_on(open_cli_secret_store_async(secret_root))
}
