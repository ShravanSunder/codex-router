//! Explicit deterministic test fixture support; never enable this feature in product builds.

use std::ffi::OsString;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::encrypted_credential_store::EncryptedCredentialStore;
pub use crate::file_backend::FileReadTrace;
pub use crate::file_backend::FileReadTraceEvent;
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
    let home_root = production_home_root()?;
    open_encrypted_credential_store_for_home(secret_root.as_ref(), &home_root)
}

fn open_encrypted_credential_store_for_home(
    secret_root: &Path,
    home_root: &Path,
) -> Result<EncryptedCredentialStore, SecretStoreError> {
    ensure_test_credential_key_root_safe_for_home(secret_root, home_root)?;
    let file_store = FileSecretStore::open(secret_root)?;
    let data_key = PooledCredentialDataKey::from_bytes([0x54; 32]);
    Ok(EncryptedCredentialStore::new(file_store, data_key))
}

/// Opens an encrypted fixture store and records temporary writes, renames and deletions.
pub fn open_encrypted_credential_store_with_write_trace(
    secret_root: impl AsRef<Path>,
) -> Result<(EncryptedCredentialStore, FileWriteTrace), SecretStoreError> {
    let home_root = production_home_root()?;
    open_encrypted_credential_store_with_write_trace_for_home(secret_root.as_ref(), &home_root)
}

/// Opens an encrypted fixture store and records file/directory reads plus file writes.
pub fn open_encrypted_credential_store_with_read_write_traces(
    secret_root: impl AsRef<Path>,
) -> Result<(EncryptedCredentialStore, FileReadTrace, FileWriteTrace), SecretStoreError> {
    let home_root = production_home_root()?;
    open_encrypted_credential_store_with_read_write_traces_for_home(
        secret_root.as_ref(),
        &home_root,
    )
}

fn open_encrypted_credential_store_with_write_trace_for_home(
    secret_root: &Path,
    home_root: &Path,
) -> Result<(EncryptedCredentialStore, FileWriteTrace), SecretStoreError> {
    ensure_test_credential_key_root_safe_for_home(secret_root, home_root)?;
    let trace = FileWriteTrace::default();
    let file_store = FileSecretStore::open_with_write_trace(secret_root, trace.clone())?;
    let data_key = PooledCredentialDataKey::from_bytes([0x54; 32]);
    Ok((EncryptedCredentialStore::new(file_store, data_key), trace))
}

fn open_encrypted_credential_store_with_read_write_traces_for_home(
    secret_root: &Path,
    home_root: &Path,
) -> Result<(EncryptedCredentialStore, FileReadTrace, FileWriteTrace), SecretStoreError> {
    ensure_test_credential_key_root_safe_for_home(secret_root, home_root)?;
    let read_trace = FileReadTrace::default();
    let write_trace = FileWriteTrace::default();
    let file_store = FileSecretStore::open_with_read_and_write_traces(
        secret_root,
        read_trace.clone(),
        write_trace.clone(),
    )?;
    let data_key = PooledCredentialDataKey::from_bytes([0x54; 32]);
    Ok((
        EncryptedCredentialStore::new(file_store, data_key),
        read_trace,
        write_trace,
    ))
}

/// Refuses deterministic test-key access to Router's production secret directory.
pub fn ensure_test_credential_key_root_safe(secret_root: &Path) -> Result<(), SecretStoreError> {
    let home_root = production_home_root()?;
    ensure_test_credential_key_root_safe_for_home(secret_root, &home_root)
}

/// Refuses deterministic test-key access to the supplied home directory's Router root.
pub fn ensure_test_credential_key_root_safe_for_home(
    secret_root: &Path,
    home_root: &Path,
) -> Result<(), SecretStoreError> {
    let production_secret_root = home_root.join(".codex-router").join("secrets");
    let secret_root = canonicalize_path_with_missing_tail(secret_root)?;
    let production_secret_root = canonicalize_path_with_missing_tail(&production_secret_root)?;
    if secret_root == production_secret_root {
        return Err(SecretStoreError::TestCredentialKeyOnProductionRoot);
    }
    Ok(())
}

fn production_home_root() -> Result<PathBuf, SecretStoreError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(SecretStoreError::TestCredentialKeyRootUnverifiable)
}

fn canonicalize_path_with_missing_tail(path: &Path) -> Result<PathBuf, SecretStoreError> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| SecretStoreError::Filesystem {
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let mut normalized_path = PathBuf::new();
    for component in absolute_path.components() {
        match component {
            Component::Prefix(prefix) => normalized_path.push(prefix.as_os_str()),
            Component::RootDir => normalized_path.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized_path.pop();
            }
            Component::Normal(name) => normalized_path.push(name),
        }
    }

    let mut ancestor = normalized_path;
    let mut missing_tail = Vec::<OsString>::new();
    loop {
        match std::fs::canonicalize(&ancestor) {
            Ok(mut resolved_ancestor) => {
                for component in missing_tail.iter().rev() {
                    resolved_ancestor.push(component);
                }
                return Ok(resolved_ancestor);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let Some(component) = ancestor.file_name() else {
                    return Err(SecretStoreError::TestCredentialKeyRootUnverifiable);
                };
                missing_tail.push(component.to_os_string());
                if !ancestor.pop() {
                    return Err(SecretStoreError::TestCredentialKeyRootUnverifiable);
                }
            }
            Err(source) => {
                return Err(SecretStoreError::Filesystem {
                    path: ancestor,
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_key_entry_points_refuse_the_production_default_root() {
        let home = TempDir::new().expect("temporary home");
        let secret_root = home.path().join(".codex-router").join("secrets");

        let opened = open_encrypted_credential_store_for_home(&secret_root, home.path());
        let traced =
            open_encrypted_credential_store_with_write_trace_for_home(&secret_root, home.path());
        let read_and_write_traced = open_encrypted_credential_store_with_read_write_traces_for_home(
            &secret_root,
            home.path(),
        );

        assert!(matches!(
            opened,
            Err(SecretStoreError::TestCredentialKeyOnProductionRoot)
        ));
        assert!(matches!(
            traced,
            Err(SecretStoreError::TestCredentialKeyOnProductionRoot)
        ));
        assert!(matches!(
            read_and_write_traced,
            Err(SecretStoreError::TestCredentialKeyOnProductionRoot)
        ));
        assert!(!secret_root.exists());
    }

    #[test]
    fn test_key_root_guard_resolves_symlink_aliases() {
        let home = TempDir::new().expect("temporary home");
        let production_root = home.path().join(".codex-router").join("secrets");
        std::fs::create_dir_all(&production_root).expect("temporary production root");
        let alias_directory = home.path().join("secrets-alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&production_root, &alias_directory).expect("temporary alias");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&production_root, &alias_directory)
            .expect("temporary alias");
        let alias_root = alias_directory.join(".");

        let result = ensure_test_credential_key_root_safe_for_home(&alias_root, home.path());

        assert!(matches!(
            result,
            Err(SecretStoreError::TestCredentialKeyOnProductionRoot)
        ));
    }
}
