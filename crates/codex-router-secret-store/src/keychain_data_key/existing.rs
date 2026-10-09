use zeroize::Zeroizing;

use super::KeychainAccess;
use super::KeychainAccessError;
use super::POOLED_KEY_ACCOUNT_PREFIX;
use super::PooledCredentialDataKey;
use super::PooledCredentialStoreId;
use super::ROUTER_KEYCHAIN_SERVICE;
use super::data_key_from_secret;
use crate::file_backend::FileSecretStore;
use crate::model::SecretStoreError;

/// Reads the store-bound key without creating an id or Keychain item.
pub(crate) fn load_existing_pooled_credential_data_key(
    file_store: &FileSecretStore,
    keychain: &dyn KeychainAccess,
) -> Result<PooledCredentialDataKey, SecretStoreError> {
    let store_id_value = file_store.read_store_id_file()?.ok_or_else(|| {
        SecretStoreError::InvalidCredentialStoreMarker {
            path: file_store.root().join("store-id"),
        }
    })?;
    let store_id = PooledCredentialStoreId::parse(&store_id_value, file_store)?;
    let keychain_account = format!("{POOLED_KEY_ACCOUNT_PREFIX}{store_id}");
    let secret = keychain
        .read_secret_without_user_interaction(ROUTER_KEYCHAIN_SERVICE, &keychain_account)
        .map_err(map_existing_keychain_error)?
        .ok_or(SecretStoreError::KeyMissing)?;

    data_key_from_secret(Zeroizing::new(secret))
}

fn map_existing_keychain_error(error: KeychainAccessError) -> SecretStoreError {
    match error {
        KeychainAccessError::ServiceRejected => SecretStoreError::KeychainServiceRejected,
        KeychainAccessError::Unavailable => SecretStoreError::KeyUnavailable,
        KeychainAccessError::TestKeychainAccessForbidden => {
            SecretStoreError::TestKeychainAccessForbidden
        }
    }
}
