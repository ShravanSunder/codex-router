//! CLI runtime secret-store factory.

use std::path::Path;

use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::SecretStoreError;

/// CLI runtime secret-store backend.
pub(crate) type CliRuntimeSecretStore = FileSecretStore;

/// Opens the CLI runtime secret store.
pub(crate) fn open_cli_secret_store(
    secret_root: &Path,
) -> Result<CliRuntimeSecretStore, SecretStoreError> {
    FileSecretStore::open(secret_root)
}
