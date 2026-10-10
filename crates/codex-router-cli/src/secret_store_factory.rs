//! CLI runtime secret-store factory.

use std::path::Path;
use std::path::PathBuf;

#[cfg(not(any(test, feature = "keychain-test-support")))]
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::model::SecretStoreError;
use thiserror::Error;

/// CLI runtime secret-store backend.
pub(crate) type CliRuntimeSecretStore =
    codex_router_secret_store::runtime_credential_store::RuntimeCredentialStore;

#[cfg(debug_assertions)]
pub(crate) fn validate_plaintext_debug_path(path: &Path) -> Result<(), crate::CliError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(crate::CliError::CredentialStoreOpen)?;
    codex_native_integration::validate_debug_directory(path, &home.join(".codex-router"))
        .map_err(|_| crate::CliError::CredentialStoreOpen)
}

#[cfg(debug_assertions)]
pub(crate) fn initialize_plaintext_debug_root(secret_root: &Path) -> Result<(), SecretStoreError> {
    codex_router_secret_store::file_backend::FileSecretStore::initialize_debug_plaintext(
        secret_root,
    )?;
    Ok(())
}

/// Opens the CLI runtime secret store.
pub(crate) fn open_cli_secret_store(
    secret_root: &Path,
) -> Result<CliRuntimeSecretStore, SecretStoreError> {
    if let Some(store) =
        codex_router_secret_store::file_backend::FileSecretStore::open_declared_debug_plaintext(
            secret_root,
        )?
    {
        #[cfg(debug_assertions)]
        return Ok(CliRuntimeSecretStore::DebugPlaintext(store));
        #[cfg(not(debug_assertions))]
        {
            let _ = store;
            return Err(SecretStoreError::DebugPlaintextUnavailable);
        }
    }
    #[cfg(any(test, feature = "keychain-test-support"))]
    {
        codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
            .map(Into::into)
    }
    #[cfg(not(any(test, feature = "keychain-test-support")))]
    {
        EncryptedCredentialStore::open_production_for_process(secret_root).map(Into::into)
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
    #[error(transparent)]
    SecretStore(#[from] SecretStoreError),
    #[error("credential store construction task failed")]
    Task(#[source] tokio::task::JoinError),
}
