//! Proxy runtime secret-store factory.

#[cfg(test)]
use codex_router_secret_store::model::SecretStoreError;
use codex_router_secret_store::runtime_credential_store::RuntimeCredentialStore;
#[cfg(test)]
use std::path::Path;

/// Proxy runtime secret-store backend.
pub(crate) type ProxyRuntimeSecretStore = RuntimeCredentialStore;

/// Opens the proxy runtime secret store.
#[cfg(test)]
pub(crate) fn open_proxy_secret_store(
    secret_root: &Path,
) -> Result<ProxyRuntimeSecretStore, SecretStoreError> {
    #[cfg(debug_assertions)]
    if let Some(store) =
        codex_router_secret_store::file_backend::FileSecretStore::open_declared_debug_plaintext(
            secret_root,
        )?
    {
        return Ok(RuntimeCredentialStore::DebugPlaintext(store));
    }
    codex_router_secret_store::file_backend::FileSecretStore::reject_debug_plaintext_declaration(
        secret_root,
    )?;
    codex_router_secret_store::test_support::open_encrypted_credential_store(secret_root)
        .map(Into::into)
}
