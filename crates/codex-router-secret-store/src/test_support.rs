//! Explicit deterministic test fixture support; never enable this feature in product builds.

use std::path::Path;

use crate::encrypted_credential_store::EncryptedCredentialStore;
use crate::file_backend::FileSecretStore;
pub use crate::file_backend::FileWriteTrace;
pub use crate::file_backend::FileWriteTraceEvent;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::KeychainAccessError;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::keychain_data_key::ROUTER_KEYCHAIN_SERVICE;
use crate::model::SecretStoreError;

/// Deterministic Keychain stand-in for Host migration tests.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeterministicTestKeychainAccess;

impl KeychainAccess for DeterministicTestKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        Ok(Some(vec![0x54; 32]))
    }

    fn add_secret(
        &self,
        service: &str,
        _account: &str,
        _secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        if service != ROUTER_KEYCHAIN_SERVICE {
            return Err(KeychainAccessError::ServiceRejected);
        }
        Err(KeychainAccessError::TestKeychainAccessForbidden)
    }
}

/// Opens an encrypted file store with a deterministic fixture-only data key.
pub fn open_encrypted_credential_store(
    secret_root: impl AsRef<Path>,
) -> Result<EncryptedCredentialStore, SecretStoreError> {
    let file_store = FileSecretStore::open(secret_root)?;
    let data_key = PooledCredentialDataKey::from_bytes([0x54; 32]);
    Ok(EncryptedCredentialStore::new(file_store, data_key))
}

/// Opens an encrypted fixture store and records temporary writes, renames and deletions.
pub fn open_encrypted_credential_store_with_write_trace(
    secret_root: impl AsRef<Path>,
) -> Result<(EncryptedCredentialStore, FileWriteTrace), SecretStoreError> {
    let trace = FileWriteTrace::default();
    let file_store = FileSecretStore::open_with_write_trace(secret_root, trace.clone())?;
    let data_key = PooledCredentialDataKey::from_bytes([0x54; 32]);
    Ok((EncryptedCredentialStore::new(file_store, data_key), trace))
}
