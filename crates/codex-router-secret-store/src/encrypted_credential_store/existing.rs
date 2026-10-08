use std::path::Path;

use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::load_existing_pooled_credential_data_key;
use crate::keychain_data_key::with_platform_keychain_for_production;
use crate::model::CredentialMigrationFailure;
use crate::model::SecretStoreError;

use super::EncryptedCredentialStore;

impl EncryptedCredentialStore {
    /// Opens an existing pooled store through the noninteractive production Keychain read.
    pub fn open_existing_production_for_process(
        secret_root: impl AsRef<Path>,
    ) -> Result<Self, SecretStoreError> {
        with_platform_keychain_for_production(|keychain| {
            Self::open_existing_for_process_with_keychain(secret_root, keychain)
        })
    }

    /// Opens and validates existing pooled-store metadata without creating or repairing it.
    ///
    /// The returned cached handle is not a read-only capability; callers must keep
    /// Prepare effects separate from later writes that use this handle.
    pub fn open_existing_for_process_with_keychain(
        secret_root: impl AsRef<Path>,
        keychain: &dyn KeychainAccess,
    ) -> Result<Self, SecretStoreError> {
        let file_store = FileSecretStore::open_read_only(secret_root)?;
        let data_key = match load_existing_pooled_credential_data_key(&file_store, keychain) {
            Ok(data_key) => data_key,
            Err(error @ (SecretStoreError::KeyUnavailable | SecretStoreError::KeyMissing)) => {
                tracing::error!(reason = %error, "existing pooled credential Keychain key is unavailable");
                return Ok(Self::key_unavailable(file_store));
            }
            Err(
                error @ (SecretStoreError::InvalidDataKey
                | SecretStoreError::InvalidCredentialStoreMarker { .. }
                | SecretStoreError::KeychainServiceRejected),
            ) => {
                tracing::error!(reason = %error, "existing pooled credential key metadata is unavailable");
                return Ok(Self::key_unavailable(file_store));
            }
            Err(error) => {
                if let Some(failure) = super::migration_failure_for_key_open_error(&error) {
                    tracing::error!(reason = %error, "existing pooled credential migration metadata is unavailable");
                    return Ok(Self::migration_incomplete(file_store, Vec::new(), failure));
                }
                return Err(error);
            }
        };

        let migration_accounts = match file_store.list_migration_account_names() {
            Ok(accounts) => accounts,
            Err(error) => {
                tracing::error!(reason = %error, "existing pooled credential migration metadata could not be read");
                return Ok(Self::migration_incomplete(
                    file_store,
                    Vec::new(),
                    super::migration_metadata_failure(&error),
                ));
            }
        };
        let marker_is_complete = match file_store.has_format_v2_marker() {
            Ok(marker_is_complete) => marker_is_complete,
            Err(error @ SecretStoreError::InvalidCredentialStoreMarker { .. }) => {
                tracing::error!(reason = %error, "existing pooled credential marker is invalid");
                return Ok(Self::migration_incomplete(
                    file_store,
                    migration_accounts,
                    CredentialMigrationFailure::InvalidStoreData,
                ));
            }
            Err(error) => {
                tracing::error!(reason = %error, "existing pooled credential marker could not be read");
                return Ok(Self::migration_incomplete(
                    file_store,
                    migration_accounts,
                    super::migration_metadata_failure(&error),
                ));
            }
        };
        let legacy_files = match file_store.list_pooled_credential_files("secret") {
            Ok(files) => files,
            Err(error) => {
                tracing::error!(reason = %error, "existing legacy credential metadata could not be read");
                return Ok(Self::migration_incomplete(
                    file_store,
                    migration_accounts,
                    super::migration_metadata_failure(&error),
                ));
            }
        };
        let _encrypted_files = match file_store.list_pooled_credential_files("v2") {
            Ok(files) => files,
            Err(error) => {
                tracing::error!(reason = %error, "existing encrypted credential metadata could not be read");
                return Ok(Self::migration_incomplete(
                    file_store,
                    migration_accounts,
                    super::migration_metadata_failure(&error),
                ));
            }
        };
        let temporary_files = match file_store.list_pooled_credential_temporary_files() {
            Ok(files) => files,
            Err(error) => {
                tracing::error!(reason = %error, "existing credential temporary-file metadata could not be read");
                return Ok(Self::migration_incomplete(
                    file_store,
                    migration_accounts,
                    super::migration_metadata_failure(&error),
                ));
            }
        };

        if marker_is_complete && legacy_files.is_empty() && temporary_files.is_empty() {
            return Ok(Self::new(file_store, data_key));
        }

        let failure = match (
            marker_is_complete,
            legacy_files.is_empty(),
            temporary_files.is_empty(),
        ) {
            (false, _, _) => CredentialMigrationFailure::MigrationNotComplete,
            (true, false, _) => CredentialMigrationFailure::InvalidStoreData,
            (true, true, false) => CredentialMigrationFailure::UnexpectedEntry,
            (true, true, true) => CredentialMigrationFailure::InvalidStoreData,
        };
        Ok(Self::migration_incomplete(
            file_store,
            migration_accounts,
            failure,
        ))
    }
}
