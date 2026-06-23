//! Proxy runtime secret-store factory.

use std::path::Path;

use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_secret_store::model::SecretStoreError;

/// Proxy runtime secret-store backend.
pub(crate) type ProxyRuntimeSecretStore = FileSecretStore;

/// Opens the proxy runtime secret store.
pub(crate) fn open_proxy_secret_store(
    secret_root: &Path,
) -> Result<ProxyRuntimeSecretStore, SecretStoreError> {
    FileSecretStore::open(secret_root)
}
