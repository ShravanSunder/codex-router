//! Encrypted pooled-credential files and migration-only legacy access.

use std::fs;
use std::path::PathBuf;

use codex_router_core::redaction::SecretString;

use super::{FileSecretStore, reject_symlink_path};
use crate::credential_key::AccountCredentialKey;
use crate::credential_key::has_credential_bundle_marker;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// A pooled credential's atomic-write temporary file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PooledCredentialTemporaryFile {
    pub key: SecretKey,
    pub path: PathBuf,
}

impl FileSecretStore {
    fn encrypted_credential_path(&self, key: &SecretKey) -> PathBuf {
        self.root.join(format!("{}.v2", key.as_str()))
    }

    /// Writes an authenticated envelope to `<key>.v2` with private temp-file permissions.
    pub(crate) fn write_credential_envelope(
        &self,
        key: &SecretKey,
        envelope: &[u8],
    ) -> Result<(), SecretStoreError> {
        require_credential_key(key)?;
        let target_path = self.encrypted_credential_path(key);
        self.write_atomically(&target_path, key.as_str(), envelope)
    }

    /// Reads only the versioned encrypted envelope for a pooled credential.
    pub(crate) fn read_credential_envelope(
        &self,
        key: &SecretKey,
    ) -> Result<Vec<u8>, SecretStoreError> {
        require_credential_key(key)?;
        let path = self.encrypted_credential_path(key);
        reject_symlink_path(&path)?;
        self.read_file_bytes(&path)
    }

    /// Deletes one encrypted generation bundle if it exists.
    pub(crate) fn delete_credential_envelope(
        &self,
        key: &SecretKey,
    ) -> Result<(), SecretStoreError> {
        require_credential_key(key)?;
        let path = self.encrypted_credential_path(key);
        reject_symlink_path(&path)?;
        match fs::remove_file(&path) {
            Ok(()) => self.record_file_removed(&path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(source) => {
                return Err(SecretStoreError::Filesystem { path, source });
            }
        }
        Ok(())
    }

    /// Reads legacy plaintext only for the startup migration.
    pub(crate) fn read_legacy_credential_for_migration(
        &self,
        key: &SecretKey,
    ) -> Result<SecretString, SecretStoreError> {
        require_credential_key(key)?;
        let path = self.secret_path(key);
        reject_symlink_path(&path)?;
        self.read_file_to_string(&path).map(SecretString::new)
    }

    /// Deletes one legacy credential after its encrypted replacement was verified.
    pub(crate) fn delete_legacy_credential_after_verification(
        &self,
        key: &SecretKey,
    ) -> Result<(), SecretStoreError> {
        require_credential_key(key)?;
        let path = self.secret_path(key);
        reject_symlink_path(&path)?;
        fs::remove_file(&path).map_err(|source| SecretStoreError::Filesystem {
            path: path.clone(),
            source,
        })?;
        self.record_file_removed(&path);
        Ok(())
    }

    /// Lists recognized pooled credential files with the requested suffix.
    pub(crate) fn list_pooled_credential_files(
        &self,
        suffix: &str,
    ) -> Result<Vec<SecretKey>, SecretStoreError> {
        let entries = self.read_directory(&self.root)?;
        let file_suffix = format!(".{suffix}");
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(key_name) = file_name.strip_suffix(&file_suffix) else {
                continue;
            };
            let key = SecretKey::new(key_name.to_owned())?;
            if !has_credential_bundle_marker(&key) {
                continue;
            }
            reject_symlink_path(&path)?;
            if !entry
                .file_type()
                .map_err(|source| SecretStoreError::Filesystem {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(SecretStoreError::UnexpectedCredentialEntry { path });
            }
            if AccountCredentialKey::parse(&key)?.is_none() {
                return Err(SecretStoreError::InvalidCredentialKey {
                    key: key.as_str().to_owned(),
                });
            }
            keys.push(key);
        }
        keys.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(keys)
    }

    /// Lists encrypted credential candidates for best-effort generation pruning.
    ///
    /// Unlike migration scans, pruning must not stop because an unrelated `.v2`
    /// credential filename is malformed. The pruning caller parses each key and
    /// warns before skipping any invalid credential identity.
    pub(crate) fn list_pooled_credential_files_for_pruning(
        &self,
    ) -> Result<Vec<SecretKey>, SecretStoreError> {
        let entries = self.read_directory(&self.root)?;
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(key_name) = file_name.strip_suffix(".v2") else {
                continue;
            };
            if !key_name.contains("_credential_bundle") {
                continue;
            }
            reject_symlink_path(&path)?;
            if !entry
                .file_type()
                .map_err(|source| SecretStoreError::Filesystem {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(SecretStoreError::UnexpectedCredentialEntry { path });
            }
            match SecretKey::new(key_name.to_owned()) {
                Ok(key) => keys.push(key),
                Err(error) => {
                    tracing::warn!(
                        file_name,
                        reason = %error,
                        "unparseable encrypted credential filename skipped during pruning"
                    );
                }
            }
        }
        keys.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        Ok(keys)
    }

    /// Conservatively detects any v2 envelope before a missing-key decision.
    pub(crate) fn has_any_v2_files(&self) -> Result<bool, SecretStoreError> {
        let entries = self.read_directory(&self.root)?;
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "v2") {
                reject_symlink_path(&path)?;
                if !entry
                    .file_type()
                    .map_err(|source| SecretStoreError::Filesystem {
                        path: path.clone(),
                        source,
                    })?
                    .is_file()
                {
                    return Err(SecretStoreError::UnexpectedCredentialEntry { path });
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Lists credential account labels leniently for unavailable-store diagnostics.
    pub(crate) fn list_migration_account_names(&self) -> Result<Vec<String>, SecretStoreError> {
        let entries = self.read_directory(&self.root)?;
        let mut account_names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let file_key_name = [".secret", ".v2"]
                .iter()
                .find_map(|suffix| file_name.strip_suffix(suffix));
            let temporary_key_name = file_name
                .strip_prefix('.')
                .and_then(|temporary_name| temporary_name.rsplit_once(".tmp."))
                .and_then(|(key_name, suffix)| {
                    let valid_suffix =
                        suffix.split_once('.').is_some_and(|(process_id, counter)| {
                            !process_id.is_empty()
                                && !counter.is_empty()
                                && process_id.bytes().all(|byte| byte.is_ascii_digit())
                                && counter.bytes().all(|byte| byte.is_ascii_digit())
                        });
                    valid_suffix.then_some(key_name)
                });
            let file_key_name = file_key_name.or(temporary_key_name);
            let Some(file_key_name) = file_key_name else {
                continue;
            };
            if !file_key_name.contains("_credential_bundle") {
                continue;
            }
            reject_symlink_path(&path)?;
            if !entry
                .file_type()
                .map_err(|source| SecretStoreError::Filesystem {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(SecretStoreError::UnexpectedCredentialEntry { path });
            }
            let name = match SecretKey::new(file_key_name.to_owned())
                .ok()
                .and_then(|key| AccountCredentialKey::parse(&key).ok().flatten())
            {
                Some(key) => key.account_id().as_str().to_owned(),
                None => file_key_name.to_owned(),
            };
            account_names.push(name);
        }
        account_names.sort();
        account_names.dedup();
        Ok(account_names)
    }

    /// Finds leftover atomic-write files for pooled credential keys.
    pub(crate) fn list_pooled_credential_temporary_files(
        &self,
    ) -> Result<Vec<PooledCredentialTemporaryFile>, SecretStoreError> {
        let entries = self.read_directory(&self.root)?;
        let mut files = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(temp_name) = file_name.strip_prefix('.') else {
                continue;
            };
            let Some((key_name, suffix)) = temp_name.rsplit_once(".tmp.") else {
                continue;
            };
            let Some((process_id, counter)) = suffix.split_once('.') else {
                continue;
            };
            if process_id.is_empty()
                || counter.is_empty()
                || !process_id.bytes().all(|byte| byte.is_ascii_digit())
                || !counter.bytes().all(|byte| byte.is_ascii_digit())
                || !key_name.contains("_credential_bundle")
            {
                continue;
            }
            let key = SecretKey::new(key_name.to_owned())?;
            if AccountCredentialKey::parse(&key)?.is_none() {
                return Err(SecretStoreError::InvalidCredentialKey {
                    key: key.as_str().to_owned(),
                });
            }
            reject_symlink_path(&path)?;
            if !entry
                .file_type()
                .map_err(|source| SecretStoreError::Filesystem {
                    path: path.clone(),
                    source,
                })?
                .is_file()
            {
                return Err(SecretStoreError::UnexpectedCredentialEntry { path });
            }
            files.push(PooledCredentialTemporaryFile { key, path });
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    /// Removes one previously enumerated temporary file after its generation is safe.
    pub(crate) fn remove_pooled_credential_temporary_file(
        &self,
        temporary_file: &PooledCredentialTemporaryFile,
    ) -> Result<(), SecretStoreError> {
        if temporary_file.path.parent() != Some(self.root.as_path()) {
            return Err(SecretStoreError::InvalidCredentialKey {
                key: temporary_file.key.as_str().to_owned(),
            });
        }
        reject_symlink_path(&temporary_file.path)?;
        match fs::remove_file(&temporary_file.path) {
            Ok(()) => {
                self.record_file_removed(&temporary_file.path);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(SecretStoreError::Filesystem {
                path: temporary_file.path.clone(),
                source,
            }),
        }
    }
}

fn require_credential_key(key: &SecretKey) -> Result<(), SecretStoreError> {
    if has_credential_bundle_marker(key) {
        return Ok(());
    }
    Err(SecretStoreError::InvalidCredentialKey {
        key: key.as_str().to_owned(),
    })
}
