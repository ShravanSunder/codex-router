//! One typed credential backend carried through CLI and proxy composition.

use crate::SecretStore;
use crate::encrypted_credential_store::{EncryptedCredentialStore, EncryptedCredentialStoreStatus};
#[cfg(debug_assertions)]
use crate::file_backend::FileSecretStore;
use crate::model::{CredentialMigrationFailure, SecretKey, SecretStoreError};
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;

#[derive(Clone, Debug)]
/// The selected credential backend for one process, never a fallback on key failure.
pub enum RuntimeCredentialStore {
    /// Default Keychain-backed authenticated encryption.
    Encrypted(EncryptedCredentialStore),
    /// Explicit, validated root-local plaintext storage in debug builds.
    #[cfg(debug_assertions)]
    DebugPlaintext(FileSecretStore),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Backend-neutral startup state consumed by account, quota and proxy composition.
pub enum RuntimeCredentialStoreStatus {
    /// The selected backend can read and stage pooled credentials.
    Ready,
    /// The encrypted backend could not obtain its Keychain key.
    KeyUnavailable,
    /// The encrypted backend could not complete its existing migration.
    MigrationIncomplete {
        accounts: Vec<String>,
        failure: CredentialMigrationFailure,
    },
}

impl From<EncryptedCredentialStore> for RuntimeCredentialStore {
    fn from(store: EncryptedCredentialStore) -> Self {
        Self::Encrypted(store)
    }
}

impl RuntimeCredentialStore {
    /// Reports the selected backend's initialization state without reopening it.
    pub fn status(&self) -> RuntimeCredentialStoreStatus {
        match self {
            Self::Encrypted(store) => match store.status() {
                EncryptedCredentialStoreStatus::Ready => RuntimeCredentialStoreStatus::Ready,
                EncryptedCredentialStoreStatus::KeyUnavailable => {
                    RuntimeCredentialStoreStatus::KeyUnavailable
                }
                EncryptedCredentialStoreStatus::MigrationIncomplete { accounts, failure } => {
                    RuntimeCredentialStoreStatus::MigrationIncomplete { accounts, failure }
                }
            },
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(_) => RuntimeCredentialStoreStatus::Ready,
        }
    }
}

impl SecretStore for RuntimeCredentialStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        match self {
            Self::Encrypted(store) => store.write_secret(key, secret),
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(store) => store.write_secret(key, secret),
        }
    }
    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        match self {
            Self::Encrypted(store) => store.read_secret(key),
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(store) => store.read_secret(key),
        }
    }
    fn write_staged(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        match self {
            Self::Encrypted(store) => store.write_staged(key, secret),
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(store) => store.write_staged(key, secret),
        }
    }
    fn delete_staged(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        match self {
            Self::Encrypted(store) => store.delete_staged(key),
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(store) => store.delete_staged(key),
        }
    }
    fn prune_obsolete_generations(
        &self,
        provider: Provider,
        account_id: &AccountId,
        generation: u64,
    ) -> Result<Vec<u64>, SecretStoreError> {
        match self {
            Self::Encrypted(store) => {
                store.prune_obsolete_generations(provider, account_id, generation)
            }
            #[cfg(debug_assertions)]
            Self::DebugPlaintext(store) => {
                store.prune_obsolete_generations(provider, account_id, generation)
            }
        }
    }
}
