//! Proxy runtime secret-store factory.

use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
#[cfg(test)]
use codex_router_secret_store::model::SecretStoreError;
#[cfg(test)]
use std::path::Path;

/// Proxy runtime secret-store backend.
pub(crate) type ProxyRuntimeSecretStore = EncryptedCredentialStore;

/// Opens the proxy runtime secret store.
#[cfg(test)]
pub(crate) fn open_proxy_secret_store(
    secret_root: &Path,
) -> Result<ProxyRuntimeSecretStore, SecretStoreError> {
    codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
}
