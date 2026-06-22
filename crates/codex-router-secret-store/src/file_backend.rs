//! Hardened file-backed secret store.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use codex_router_core::redaction::SecretString;

use crate::backend::SecretStore;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

/// File-backed secret store rooted in router-owned private storage.
#[derive(Clone, Debug)]
pub struct FileSecretStore {
    root: PathBuf,
}

impl FileSecretStore {
    /// Opens or creates a file-backed secret store.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SecretStoreError> {
        let root = root.as_ref().to_path_buf();
        reject_codex_home_path(&root)?;
        reject_symlink_path(&root)?;
        validate_existing_parent(&root)?;
        create_private_dir(&root)?;

        Ok(Self { root })
    }

    fn secret_path(&self, key: &SecretKey) -> PathBuf {
        self.root.join(format!("{}.secret", key.as_str()))
    }
}

impl SecretStore for FileSecretStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        let target_path = self.secret_path(key);
        reject_symlink_path(&target_path)?;

        let temp_path = self
            .root
            .join(format!(".{}.tmp.{}", key.as_str(), std::process::id()));
        reject_symlink_path(&temp_path)?;

        let mut temp_file = open_private_temp_file(&temp_path)?;
        write_all(
            &mut temp_file,
            &temp_path,
            secret.expose_secret().as_bytes(),
        )?;
        sync_file(&temp_file, &temp_path)?;
        drop(temp_file);

        rename(&temp_path, &target_path)?;
        set_private_file_permissions(&target_path)?;

        Ok(())
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        let target_path = self.secret_path(key);
        reject_symlink_path(&target_path)?;
        let value = read_to_string(&target_path)?;

        Ok(SecretString::new(value))
    }
}

fn reject_codex_home_path(path: &Path) -> Result<(), SecretStoreError> {
    if path.components().any(is_codex_component) {
        return Err(SecretStoreError::CodexHomePath {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn is_codex_component(component: Component<'_>) -> bool {
    matches!(component, Component::Normal(value) if value == ".codex")
}

fn reject_symlink_path(path: &Path) -> Result<(), SecretStoreError> {
    if path_is_symlink(path)? {
        return Err(SecretStoreError::SymlinkPath {
            path: path.to_path_buf(),
        });
    }

    Ok(())
}

fn validate_existing_parent(path: &Path) -> Result<(), SecretStoreError> {
    let mut current_path = path.parent();
    while let Some(parent) = current_path {
        reject_symlink_path(parent)?;
        if parent.exists() {
            return Ok(());
        }
        current_path = parent.parent();
    }

    Ok(())
}

fn path_is_symlink(path: &Path) -> Result<bool, SecretStoreError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn create_private_dir(path: &Path) -> Result<(), SecretStoreError> {
    fs::create_dir_all(path).map_err(|source| SecretStoreError::Filesystem {
        path: path.to_path_buf(),
        source,
    })?;
    set_private_dir_permissions(path)
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), SecretStoreError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(unix)]
fn open_private_temp_file(path: &Path) -> Result<fs::File, SecretStoreError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), SecretStoreError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        }
    })
}

fn write_all(file: &mut fs::File, path: &Path, value: &[u8]) -> Result<(), SecretStoreError> {
    file.write_all(value)
        .map_err(|source| SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

fn sync_file(file: &fs::File, path: &Path) -> Result<(), SecretStoreError> {
    file.sync_all()
        .map_err(|source| SecretStoreError::Filesystem {
            path: path.to_path_buf(),
            source,
        })
}

fn rename(from_path: &Path, to_path: &Path) -> Result<(), SecretStoreError> {
    fs::rename(from_path, to_path).map_err(|source| SecretStoreError::Filesystem {
        path: to_path.to_path_buf(),
        source,
    })
}

fn read_to_string(path: &Path) -> Result<String, SecretStoreError> {
    fs::read_to_string(path).map_err(|source| SecretStoreError::Filesystem {
        path: path.to_path_buf(),
        source,
    })
}
