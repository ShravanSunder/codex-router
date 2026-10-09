use codex_router_secret_store::{
    encrypted_credential_store::EncryptedCredentialStore, model::SecretStoreError,
};
use std::path::Path;
pub(crate) fn open_role_credential_store(
    path: &Path,
) -> Result<EncryptedCredentialStore, SecretStoreError> {
    #[cfg(any(test, feature = "test-support", feature = "keychain-test-support"))]
    {
        codex_router_secret_store::test_support::open_encrypted_credential_store(path)
    }
    #[cfg(not(any(test, feature = "test-support", feature = "keychain-test-support")))]
    {
        EncryptedCredentialStore::open_production_for_process(path)
    }
}
