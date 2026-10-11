//! Resumable v2 migration for pooled credential files.

use std::collections::BTreeSet;
use std::path::Path;

use codex_router_core::redaction::SecretString;

use crate::credential_key::AccountCredentialKey;
use crate::credential_store_lock::CredentialStoreLock;
use crate::credential_store_lock::CredentialStoreLockMode;
use crate::encrypted_credential_store::decrypt_envelope;
use crate::encrypted_credential_store::encrypt_envelope;
use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::keychain_data_key::load_or_create_pooled_credential_data_key;
use crate::keychain_data_key::with_platform_keychain_for_production;
pub use crate::model::CredentialMigrationFailure;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// Safe outcome of the startup migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialMigrationOutcome {
    /// Every pooled credential is v2 encrypted and the completion marker is present.
    Complete,
    /// The migration stopped; the named accounts remain unavailable to runtime readers.
    Incomplete {
        /// Account ids that still require conversion or repair.
        accounts: Vec<String>,
        /// Safe reason suitable for a host log.
        failure: CredentialMigrationFailure,
    },
    /// The Keychain key could not be read or created for this process.
    KeyUnavailable,
}

/// Loads the key and migrates pooled credentials while holding the exclusive store lock.
pub fn migrate_pooled_credentials_at_startup(
    secret_root: impl AsRef<Path>,
    keychain: &dyn KeychainAccess,
) -> Result<CredentialMigrationOutcome, SecretStoreError> {
    FileSecretStore::reject_debug_plaintext_declaration(secret_root.as_ref())?;
    let file_store = FileSecretStore::open(secret_root)?;
    let _store_lock =
        CredentialStoreLock::acquire(file_store.root(), CredentialStoreLockMode::Exclusive)?;
    FileSecretStore::reject_debug_plaintext_declaration(file_store.root())?;
    let diagnostic_accounts = file_store
        .list_migration_account_names()
        .unwrap_or_default();
    let marker_exists = match file_store.has_format_v2_marker() {
        Ok(marker_exists) => marker_exists,
        Err(_) => {
            return Ok(incomplete(
                diagnostic_accounts,
                CredentialMigrationFailure::InvalidStoreData,
            ));
        }
    };
    let legacy_keys = match file_store.list_pooled_credential_files("secret") {
        Ok(keys) => keys,
        Err(_) => {
            return Ok(incomplete(
                diagnostic_accounts,
                CredentialMigrationFailure::InvalidStoreData,
            ));
        }
    };
    let encrypted_keys = match file_store.list_pooled_credential_files("v2") {
        Ok(keys) => keys,
        Err(_) => {
            return Ok(incomplete(
                diagnostic_accounts,
                CredentialMigrationFailure::InvalidStoreData,
            ));
        }
    };
    let temporary_files = match file_store.list_pooled_credential_temporary_files() {
        Ok(files) => files,
        Err(_) => {
            return Ok(incomplete(
                diagnostic_accounts,
                CredentialMigrationFailure::UnexpectedEntry,
            ));
        }
    };
    if remove_startup_temporary_files(&file_store, temporary_files).is_err() {
        return Ok(incomplete(
            diagnostic_accounts,
            CredentialMigrationFailure::UnexpectedEntry,
        ));
    }
    if marker_exists {
        if legacy_keys.is_empty() {
            return Ok(CredentialMigrationOutcome::Complete);
        }
        return Ok(incomplete(
            accounts_for_keys(&legacy_keys.iter().cloned().collect()),
            CredentialMigrationFailure::InvalidStoreData,
        ));
    }
    if legacy_keys.is_empty() && encrypted_keys.is_empty() {
        return Ok(match file_store.write_format_v2_marker() {
            Ok(()) => CredentialMigrationOutcome::Complete,
            Err(_) => incomplete(Vec::new(), CredentialMigrationFailure::MarkerWriteFailed),
        });
    }
    let data_key = match load_or_create_pooled_credential_data_key(&file_store, keychain) {
        Ok(data_key) => data_key,
        Err(SecretStoreError::KeyUnavailable | SecretStoreError::KeyMissing) => {
            return Ok(CredentialMigrationOutcome::KeyUnavailable);
        }
        Err(error) => return Err(error),
    };
    Ok(migrate_with_exclusive_lock(&file_store, &data_key))
}

/// Runs startup migration through the explicit production Keychain entry point.
///
/// Tests must use `migrate_pooled_credentials_at_startup` with an explicit
/// `KeychainAccess` implementation instead.
pub fn migrate_pooled_credentials_at_production_startup(
    secret_root: impl AsRef<Path>,
) -> Result<CredentialMigrationOutcome, SecretStoreError> {
    FileSecretStore::reject_debug_plaintext_declaration(secret_root.as_ref())?;
    with_platform_keychain_for_production(|keychain| {
        migrate_pooled_credentials_at_startup(secret_root, keychain)
    })
}

fn remove_startup_temporary_files(
    file_store: &FileSecretStore,
    temporary_files: Vec<crate::file_backend::PooledCredentialTemporaryFile>,
) -> Result<(), SecretStoreError> {
    for temporary_file in temporary_files {
        file_store.remove_pooled_credential_temporary_file(&temporary_file)?;
    }
    file_store.remove_orphaned_format_v2_marker_temps()
}

pub(crate) fn migrate_with_exclusive_lock(
    file_store: &FileSecretStore,
    data_key: &PooledCredentialDataKey,
) -> CredentialMigrationOutcome {
    let initial_account_names = match file_store.list_migration_account_names() {
        Ok(accounts) => accounts,
        Err(_) => return incomplete(Vec::new(), CredentialMigrationFailure::UnexpectedEntry),
    };
    let marker_exists = match file_store.has_format_v2_marker() {
        Ok(marker_exists) => marker_exists,
        Err(_) => {
            return incomplete(
                initial_account_names,
                CredentialMigrationFailure::InvalidStoreData,
            );
        }
    };
    let legacy_keys = match file_store.list_pooled_credential_files("secret") {
        Ok(keys) => keys,
        Err(_) => {
            return incomplete(
                initial_account_names,
                CredentialMigrationFailure::InvalidStoreData,
            );
        }
    };
    let encrypted_keys = match file_store.list_pooled_credential_files("v2") {
        Ok(keys) => keys,
        Err(_) => {
            return incomplete(
                initial_account_names,
                CredentialMigrationFailure::InvalidStoreData,
            );
        }
    };
    let temporary_files = match file_store.list_pooled_credential_temporary_files() {
        Ok(files) => files,
        Err(_) => {
            return incomplete(
                initial_account_names,
                CredentialMigrationFailure::UnexpectedEntry,
            );
        }
    };
    if file_store.remove_orphaned_format_v2_marker_temps().is_err() {
        return incomplete(
            initial_account_names,
            CredentialMigrationFailure::UnexpectedEntry,
        );
    }

    let all_keys = legacy_keys
        .iter()
        .chain(encrypted_keys.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    if remove_startup_temporary_files(file_store, temporary_files).is_err() {
        return incomplete(
            accounts_for_keys(&all_keys),
            CredentialMigrationFailure::UnexpectedEntry,
        );
    }

    if marker_exists {
        if legacy_keys.is_empty() {
            return CredentialMigrationOutcome::Complete;
        }
        return incomplete(
            accounts_for_keys(&legacy_keys.iter().cloned().collect()),
            CredentialMigrationFailure::InvalidStoreData,
        );
    }

    let legacy_key_set = legacy_keys.iter().cloned().collect::<BTreeSet<_>>();
    let encrypted_key_set = encrypted_keys.iter().cloned().collect::<BTreeSet<_>>();
    let ordered_keys = all_keys.iter().cloned().collect::<Vec<_>>();
    for (index, key) in ordered_keys.iter().enumerate() {
        let has_legacy = legacy_key_set.contains(key);
        let has_encrypted = encrypted_key_set.contains(key);
        if has_legacy {
            let legacy_secret = match file_store.read_legacy_credential_for_migration(key) {
                Ok(secret) => secret,
                Err(_) => {
                    return incomplete(
                        accounts_for_remaining_keys(&ordered_keys, index),
                        CredentialMigrationFailure::CredentialReadFailed,
                    );
                }
            };
            if has_encrypted {
                match read_and_verify_encrypted_credential(
                    file_store,
                    data_key,
                    key,
                    Some(&legacy_secret),
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        return incomplete(
                            accounts_for_remaining_keys(&ordered_keys, index),
                            CredentialMigrationFailure::ReadBackMismatch,
                        );
                    }
                    Err(_) => {
                        return incomplete(
                            accounts_for_remaining_keys(&ordered_keys, index),
                            CredentialMigrationFailure::CredentialReadFailed,
                        );
                    }
                }
            } else {
                let envelope =
                    match encrypt_envelope(data_key, key, legacy_secret.expose_secret().as_bytes())
                    {
                        Ok(envelope) => envelope,
                        Err(_) => {
                            return incomplete(
                                accounts_for_remaining_keys(&ordered_keys, index),
                                CredentialMigrationFailure::EnvelopeWriteFailed,
                            );
                        }
                    };
                if file_store
                    .write_credential_envelope(key, &envelope)
                    .is_err()
                {
                    return incomplete(
                        accounts_for_remaining_keys(&ordered_keys, index),
                        CredentialMigrationFailure::EnvelopeWriteFailed,
                    );
                }
                match read_and_verify_encrypted_credential(
                    file_store,
                    data_key,
                    key,
                    Some(&legacy_secret),
                ) {
                    Ok(true) => {}
                    Ok(false) => {
                        return incomplete(
                            accounts_for_remaining_keys(&ordered_keys, index),
                            CredentialMigrationFailure::ReadBackMismatch,
                        );
                    }
                    Err(_) => {
                        return incomplete(
                            accounts_for_remaining_keys(&ordered_keys, index),
                            CredentialMigrationFailure::CredentialReadFailed,
                        );
                    }
                }
            }
            if file_store
                .delete_legacy_credential_after_verification(key)
                .is_err()
            {
                return incomplete(
                    accounts_for_remaining_keys(&ordered_keys, index),
                    CredentialMigrationFailure::LegacyDeleteFailed,
                );
            }
        } else if has_encrypted {
            match read_and_verify_encrypted_credential(file_store, data_key, key, None) {
                Ok(_) => {}
                Err(_) => {
                    return incomplete(
                        accounts_for_remaining_keys(&ordered_keys, index),
                        CredentialMigrationFailure::CredentialReadFailed,
                    );
                }
            }
        }
    }

    if file_store.write_format_v2_marker().is_err() {
        return incomplete(
            accounts_for_keys(&ordered_keys.iter().cloned().collect()),
            CredentialMigrationFailure::MarkerWriteFailed,
        );
    }
    CredentialMigrationOutcome::Complete
}

fn read_and_verify_encrypted_credential(
    file_store: &FileSecretStore,
    data_key: &PooledCredentialDataKey,
    key: &SecretKey,
    legacy_secret: Option<&SecretString>,
) -> Result<bool, SecretStoreError> {
    let envelope = file_store.read_credential_envelope(key)?;
    let encrypted_secret = decrypt_envelope(data_key, key, &envelope)?;
    if let Some(legacy_secret) = legacy_secret {
        Ok(encrypted_secret.expose_secret() == legacy_secret.expose_secret())
    } else {
        Ok(true)
    }
}

fn accounts_for_keys(keys: &BTreeSet<SecretKey>) -> Vec<String> {
    let mut accounts = keys.iter().map(credential_account_name).collect::<Vec<_>>();
    accounts.sort();
    accounts.dedup();
    accounts
}

fn accounts_for_remaining_keys(keys: &[SecretKey], first_unconverted_index: usize) -> Vec<String> {
    let mut accounts = keys
        .iter()
        .skip(first_unconverted_index)
        .map(credential_account_name)
        .collect::<Vec<_>>();
    accounts.sort();
    accounts.dedup();
    accounts
}

fn credential_account_name(key: &SecretKey) -> String {
    match AccountCredentialKey::parse(key) {
        Ok(Some(credential_key)) => credential_key.account_id().as_str().to_owned(),
        Ok(None) | Err(_) => key.as_str().to_owned(),
    }
}

fn incomplete(
    mut accounts: Vec<String>,
    failure: CredentialMigrationFailure,
) -> CredentialMigrationOutcome {
    accounts.sort();
    accounts.dedup();
    CredentialMigrationOutcome::Incomplete { accounts, failure }
}
