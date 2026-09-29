//! AES-256-GCM storage for provider-scoped pooled credential generations.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use codex_router_core::redaction::SecretString;
use serde::Deserialize;
use serde::Serialize;

use crate::backend::SecretStore;
use crate::credential_key::AccountCredentialKey;
use crate::credential_key::has_credential_bundle_marker;
use crate::credential_store_lock::CredentialStoreLock;
use crate::credential_store_lock::CredentialStoreLockMode;
use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::keychain_data_key::load_or_create_pooled_credential_data_key;
use crate::keychain_data_key::with_platform_keychain_for_production;
use crate::model::SecretKey;
use crate::model::SecretStoreError;
use crate::model::StoreUnavailable;

const ENCRYPTED_CREDENTIAL_FORMAT: u8 = 2;
const AES_GCM_NONCE_LENGTH: usize = 12;

/// Process-wide visibility of one pooled credential store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncryptedCredentialStoreStatus {
    /// Credential reads and staged writes use the cached data key.
    Ready,
    /// The Keychain key could not be read at this process start.
    KeyUnavailable,
    /// The startup migration stopped before every credential was verified.
    MigrationIncomplete { accounts: Vec<String> },
}

/// File-backed general secrets plus authenticated encryption for pooled credentials.
#[derive(Clone, Debug)]
pub struct EncryptedCredentialStore {
    file_store: FileSecretStore,
    state: CredentialStoreState,
}

#[derive(Clone, Debug)]
enum CredentialStoreState {
    Ready(PooledCredentialDataKey),
    KeyUnavailable,
    MigrationIncomplete { accounts: Vec<String> },
}

impl EncryptedCredentialStore {
    /// Opens the pooled credential store through the production Keychain entry point.
    pub fn open_production_for_process(
        secret_root: impl AsRef<std::path::Path>,
    ) -> Result<Self, SecretStoreError> {
        with_platform_keychain_for_production(|keychain| {
            Self::open_for_process_with_keychain(secret_root, keychain)
        })
    }

    /// Opens the process handle through an injected Keychain boundary.
    pub fn open_for_process_with_keychain(
        secret_root: impl AsRef<std::path::Path>,
        keychain: &dyn KeychainAccess,
    ) -> Result<Self, SecretStoreError> {
        let file_store = FileSecretStore::open(secret_root)?;
        let _store_lock =
            CredentialStoreLock::acquire(file_store.root(), CredentialStoreLockMode::Exclusive)?;
        let data_key = match load_or_create_pooled_credential_data_key(&file_store, keychain) {
            Ok(data_key) => data_key,
            Err(SecretStoreError::KeyUnavailable | SecretStoreError::KeyMissing) => {
                return Ok(Self::key_unavailable(file_store));
            }
            Err(error) => return Err(error),
        };
        let migration_accounts = file_store
            .list_migration_account_names()
            .unwrap_or_default();
        let marker_is_complete = file_store.has_format_v2_marker().unwrap_or(false);
        let legacy_files_exist = file_store.has_any_legacy_credential_files()?;
        let temporary_files_are_clear = file_store
            .list_pooled_credential_temporary_files()
            .is_ok_and(|files| files.is_empty());
        if marker_is_complete && !legacy_files_exist && temporary_files_are_clear {
            Ok(Self::new(file_store, data_key))
        } else {
            Ok(Self::migration_incomplete(file_store, migration_accounts))
        }
    }

    /// Creates a ready store using the key already read at this process start.
    #[must_use]
    pub fn new(file_store: FileSecretStore, data_key: PooledCredentialDataKey) -> Self {
        Self {
            file_store,
            state: CredentialStoreState::Ready(data_key),
        }
    }

    /// Creates a store that can serve non-pooled secrets while Keychain is unavailable.
    #[must_use]
    pub fn key_unavailable(file_store: FileSecretStore) -> Self {
        Self {
            file_store,
            state: CredentialStoreState::KeyUnavailable,
        }
    }

    /// Creates a store that keeps non-pooled secrets available after migration stops.
    #[must_use]
    pub fn migration_incomplete(file_store: FileSecretStore, mut accounts: Vec<String>) -> Self {
        accounts.sort();
        accounts.dedup();
        Self {
            file_store,
            state: CredentialStoreState::MigrationIncomplete { accounts },
        }
    }

    /// Returns the process-visible pooled credential store status.
    #[must_use]
    pub fn status(&self) -> EncryptedCredentialStoreStatus {
        match &self.state {
            CredentialStoreState::Ready(_) => EncryptedCredentialStoreStatus::Ready,
            CredentialStoreState::KeyUnavailable => EncryptedCredentialStoreStatus::KeyUnavailable,
            CredentialStoreState::MigrationIncomplete { accounts, .. } => {
                EncryptedCredentialStoreStatus::MigrationIncomplete {
                    accounts: accounts.clone(),
                }
            }
        }
    }

    /// Writes one inactive credential generation as a versioned encrypted envelope.
    ///
    /// This method does not activate the credential generation in account state.
    pub fn write_staged(
        &self,
        key: &SecretKey,
        secret: &SecretString,
    ) -> Result<(), SecretStoreError> {
        require_account_credential_key(key)?;
        let data_key = self.ready_data_key()?;
        let envelope = encrypt_envelope(data_key, key, secret.expose_secret().as_bytes())?;
        let _store_lock =
            CredentialStoreLock::acquire(self.file_store.root(), CredentialStoreLockMode::Shared)?;
        self.file_store.write_credential_envelope(key, &envelope)
    }

    fn ready_data_key(&self) -> Result<&PooledCredentialDataKey, SecretStoreError> {
        match &self.state {
            CredentialStoreState::Ready(data_key) => Ok(data_key),
            CredentialStoreState::KeyUnavailable => Err(SecretStoreError::KeyUnavailable),
            CredentialStoreState::MigrationIncomplete { accounts, .. } => Err(
                SecretStoreError::StoreUnavailable(StoreUnavailable::MigrationIncomplete {
                    accounts: accounts.clone(),
                }),
            ),
        }
    }
}

impl SecretStore for EncryptedCredentialStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        if has_credential_bundle_marker(key) {
            return self.write_staged(key, secret);
        }
        self.file_store.write_secret(key, secret)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        if !has_credential_bundle_marker(key) {
            return self.file_store.read_secret(key);
        }
        require_account_credential_key(key)?;
        let data_key = self.ready_data_key()?;
        let envelope = self.file_store.read_credential_envelope(key)?;
        decrypt_envelope(data_key, key, &envelope)
    }

    fn write_staged(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        EncryptedCredentialStore::write_staged(self, key, secret)
    }
}

fn require_account_credential_key(
    key: &SecretKey,
) -> Result<AccountCredentialKey, SecretStoreError> {
    AccountCredentialKey::parse(key)?.ok_or_else(|| SecretStoreError::InvalidCredentialKey {
        key: key.as_str().to_owned(),
    })
}

pub(crate) fn encrypt_envelope(
    data_key: &PooledCredentialDataKey,
    secret_key: &SecretKey,
    plaintext: &[u8],
) -> Result<Vec<u8>, SecretStoreError> {
    use aes_gcm::Aes256Gcm;
    use aes_gcm::Nonce;
    use aes_gcm::aead::Aead;
    use aes_gcm::aead::KeyInit;
    use aes_gcm::aead::Payload;
    use aes_gcm::aead::consts::U12;

    let cipher = Aes256Gcm::new_from_slice(data_key.bytes())
        .map_err(|_| SecretStoreError::CipherInitializationFailed)?;
    let mut nonce_bytes = [0_u8; AES_GCM_NONCE_LENGTH];
    getrandom::fill(&mut nonce_bytes).map_err(SecretStoreError::RandomnessUnavailable)?;
    let nonce: Nonce<U12> = Nonce::from(nonce_bytes);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: secret_key.as_str().as_bytes(),
            },
        )
        .map_err(|_| SecretStoreError::CredentialAuthenticationFailed)?;
    let envelope = EncryptedCredentialEnvelope {
        format: ENCRYPTED_CREDENTIAL_FORMAT,
        nonce: STANDARD.encode(nonce_bytes),
        ciphertext: STANDARD.encode(ciphertext),
    };
    serde_json::to_vec(&envelope).map_err(|_| SecretStoreError::InvalidCredentialEnvelope)
}

pub(crate) fn decrypt_envelope(
    data_key: &PooledCredentialDataKey,
    secret_key: &SecretKey,
    envelope_bytes: &[u8],
) -> Result<SecretString, SecretStoreError> {
    use aes_gcm::Aes256Gcm;
    use aes_gcm::Nonce;
    use aes_gcm::aead::Aead;
    use aes_gcm::aead::KeyInit;
    use aes_gcm::aead::Payload;
    use aes_gcm::aead::consts::U12;

    let envelope: EncryptedCredentialEnvelope = serde_json::from_slice(envelope_bytes)
        .map_err(|_| SecretStoreError::InvalidCredentialEnvelope)?;
    if envelope.format != ENCRYPTED_CREDENTIAL_FORMAT {
        return Err(SecretStoreError::InvalidCredentialEnvelope);
    }
    let nonce_bytes = STANDARD
        .decode(envelope.nonce)
        .map_err(|_| SecretStoreError::InvalidCredentialEnvelope)?;
    let nonce_array: [u8; AES_GCM_NONCE_LENGTH] = nonce_bytes
        .try_into()
        .map_err(|_| SecretStoreError::InvalidCredentialEnvelope)?;
    let ciphertext = STANDARD
        .decode(envelope.ciphertext)
        .map_err(|_| SecretStoreError::InvalidCredentialEnvelope)?;
    if ciphertext.len() < 16 {
        return Err(SecretStoreError::InvalidCredentialEnvelope);
    }
    let nonce: Nonce<U12> = Nonce::from(nonce_array);
    let cipher = Aes256Gcm::new_from_slice(data_key.bytes())
        .map_err(|_| SecretStoreError::CipherInitializationFailed)?;
    let plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: &ciphertext,
                aad: secret_key.as_str().as_bytes(),
            },
        )
        .map_err(|_| SecretStoreError::CredentialAuthenticationFailed)?;
    let plaintext =
        String::from_utf8(plaintext).map_err(|_| SecretStoreError::InvalidCredentialEncoding)?;
    Ok(SecretString::new(plaintext))
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EncryptedCredentialEnvelope {
    format: u8,
    nonce: String,
    ciphertext: String,
}
