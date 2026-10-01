//! Hardened file-backed secret store.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Arc;
#[cfg(any(test, feature = "test-support"))]
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use codex_router_core::redaction::SecretString;

use crate::backend::SecretStore;
use crate::credential_key::has_credential_bundle_marker;
use crate::model::SecretKey;
use crate::model::SecretStoreError;

mod pooled_credential_files;
pub(crate) use pooled_credential_files::PooledCredentialTemporaryFile;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// File-backed secret store rooted in router-owned private storage.
#[derive(Clone, Debug)]
pub struct FileSecretStore {
    root: PathBuf,
    #[cfg(any(test, feature = "test-support"))]
    write_trace: Option<FileWriteTrace>,
}

trait FileWriteObserver {
    fn temporary_file_written(&self, path: &Path, contents: &[u8]);
    fn file_renamed(&self, from: &Path, to: &Path);
    #[cfg(any(test, feature = "test-support"))]
    fn file_removed(&self, path: &Path);
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Default)]
pub struct FileWriteTrace(Arc<Mutex<Vec<FileWriteTraceEvent>>>);

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Eq, PartialEq)]
pub enum FileWriteTraceEvent {
    TemporaryFileWritten { path: PathBuf, contents: Vec<u8> },
    FileRenamed { from: PathBuf, to: PathBuf },
    FileRemoved { path: PathBuf },
}

#[cfg(any(test, feature = "test-support"))]
impl std::fmt::Debug for FileWriteTraceEvent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TemporaryFileWritten { path, .. } => formatter
                .debug_struct("TemporaryFileWritten")
                .field("path", path)
                .field("contents", &"[REDACTED]")
                .finish(),
            Self::FileRenamed { from, to } => formatter
                .debug_struct("FileRenamed")
                .field("from", from)
                .field("to", to)
                .finish(),
            Self::FileRemoved { path } => formatter
                .debug_struct("FileRemoved")
                .field("path", path)
                .finish(),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl std::fmt::Debug for FileWriteTrace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FileWriteTrace([REDACTED])")
    }
}

#[cfg(any(test, feature = "test-support"))]
impl FileWriteTrace {
    pub fn events(&self) -> Vec<FileWriteTraceEvent> {
        self.0
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default()
    }

    fn record(&self, event: FileWriteTraceEvent) {
        if let Ok(mut events) = self.0.lock() {
            events.push(event);
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl FileWriteObserver for FileWriteTrace {
    fn temporary_file_written(&self, path: &Path, contents: &[u8]) {
        self.record(FileWriteTraceEvent::TemporaryFileWritten {
            path: path.to_path_buf(),
            contents: contents.to_vec(),
        });
    }

    fn file_renamed(&self, from: &Path, to: &Path) {
        self.record(FileWriteTraceEvent::FileRenamed {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
    }

    #[cfg(any(test, feature = "test-support"))]
    fn file_removed(&self, path: &Path) {
        self.record(FileWriteTraceEvent::FileRemoved {
            path: path.to_path_buf(),
        });
    }
}

impl FileSecretStore {
    /// Opens or creates a file-backed secret store.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, SecretStoreError> {
        let root = root.as_ref().to_path_buf();
        reject_codex_home_path(&root)?;
        reject_symlink_path(&root)?;
        validate_existing_parent(&root)?;
        create_private_dir(&root)?;

        Ok(Self {
            root,
            #[cfg(any(test, feature = "test-support"))]
            write_trace: None,
        })
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn open_with_write_trace(
        root: impl AsRef<Path>,
        write_trace: FileWriteTrace,
    ) -> Result<Self, SecretStoreError> {
        let mut store = Self::open(root)?;
        store.write_trace = Some(write_trace);
        Ok(store)
    }

    /// Opens an existing file-backed secret store without creating or modifying its root.
    pub fn open_read_only(root: impl AsRef<Path>) -> Result<Self, SecretStoreError> {
        let root = root.as_ref().to_path_buf();
        reject_codex_home_path(&root)?;
        reject_symlink_path(&root)?;
        validate_existing_parent(&root)?;
        if !root.is_dir() {
            return Err(SecretStoreError::Filesystem {
                path: root,
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "read-only secret store root does not exist",
                ),
            });
        }

        Ok(Self {
            root,
            #[cfg(any(test, feature = "test-support"))]
            write_trace: None,
        })
    }

    fn secret_path(&self, key: &SecretKey) -> PathBuf {
        self.root.join(format!("{}.secret", key.as_str()))
    }

    /// Returns the canonical secret-root path to sibling storage components.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn write_atomically(
        &self,
        target_path: &Path,
        temp_stem: &str,
        contents: &[u8],
    ) -> Result<(), SecretStoreError> {
        #[cfg(any(test, feature = "test-support"))]
        let observer = self
            .write_trace
            .as_ref()
            .map(|trace| trace as &dyn FileWriteObserver);
        #[cfg(not(any(test, feature = "test-support")))]
        let observer: Option<&dyn FileWriteObserver> = None;
        write_atomic_file(&self.root, target_path, temp_stem, contents, observer)
    }

    /// Reads the non-secret identifier that binds the secret root to its Keychain item.
    pub(crate) fn read_store_id_file(&self) -> Result<Option<String>, SecretStoreError> {
        read_optional_metadata(&self.root, "store-id")
    }

    /// Publishes the non-secret identifier through a private temporary file and rename.
    pub(crate) fn write_store_id_file(&self, store_id: &str) -> Result<(), SecretStoreError> {
        let path = self.root.join("store-id");
        self.write_atomically(&path, "store-id", store_id.as_bytes())
    }

    /// Reports whether the completed format-v2 marker exists and contains the supported value.
    pub(crate) fn has_format_v2_marker(&self) -> Result<bool, SecretStoreError> {
        match read_optional_metadata(&self.root, "format-v2.marker")? {
            Some(value) if value == "2" || value == "2\n" => Ok(true),
            Some(_) => Err(SecretStoreError::InvalidCredentialStoreMarker {
                path: self.root.join("format-v2.marker"),
            }),
            None => Ok(false),
        }
    }

    /// Publishes the completed format-v2 marker atomically.
    pub(crate) fn write_format_v2_marker(&self) -> Result<(), SecretStoreError> {
        let path = self.root.join("format-v2.marker");
        self.write_atomically(&path, "format-v2.marker", b"2\n")
    }

    /// Removes abandoned atomic-write temps for the non-secret format marker.
    pub(crate) fn remove_orphaned_format_v2_marker_temps(&self) -> Result<(), SecretStoreError> {
        let entries = fs::read_dir(&self.root).map_err(|source| SecretStoreError::Filesystem {
            path: self.root.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| SecretStoreError::Filesystem {
                path: self.root.clone(),
                source,
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(suffix) = file_name.strip_prefix(".format-v2.marker.tmp.") else {
                continue;
            };
            let Some((process_id, counter)) = suffix.split_once('.') else {
                continue;
            };
            if process_id.is_empty()
                || counter.is_empty()
                || !process_id.bytes().all(|byte| byte.is_ascii_digit())
                || !counter.bytes().all(|byte| byte.is_ascii_digit())
            {
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
            fs::remove_file(&path).map_err(|source| SecretStoreError::Filesystem {
                path: path.clone(),
                source,
            })?;
            self.record_file_removed(&path);
        }
        Ok(())
    }

    fn record_file_removed(&self, path: &Path) {
        #[cfg(any(test, feature = "test-support"))]
        if let Some(trace) = &self.write_trace {
            trace.file_removed(path);
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = path;
    }
}

impl SecretStore for FileSecretStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        reject_pooled_credential_key(key)?;
        let target_path = self.secret_path(key);
        self.write_atomically(
            &target_path,
            key.as_str(),
            secret.expose_secret().as_bytes(),
        )
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        reject_pooled_credential_key(key)?;
        let target_path = self.secret_path(key);
        reject_symlink_path(&target_path)?;
        let value = read_to_string(&target_path)?;

        Ok(SecretString::new(value))
    }
}

fn reject_pooled_credential_key(key: &SecretKey) -> Result<(), SecretStoreError> {
    if has_credential_bundle_marker(key) {
        return Err(SecretStoreError::PooledCredentialRequiresEncryption {
            key: key.as_str().to_owned(),
        });
    }

    Ok(())
}

fn write_atomic_file(
    root: &Path,
    target_path: &Path,
    temp_stem: &str,
    contents: &[u8],
    observer: Option<&dyn FileWriteObserver>,
) -> Result<(), SecretStoreError> {
    reject_symlink_path(target_path)?;
    let temp_path = root.join(format!(
        ".{temp_stem}.tmp.{}.{}",
        std::process::id(),
        TEMP_FILE_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    reject_symlink_path(&temp_path)?;
    let mut temp_file = open_private_temp_file(&temp_path)?;
    write_all(&mut temp_file, &temp_path, contents)?;
    if let Some(observer) = observer {
        observer.temporary_file_written(&temp_path, contents);
    }
    sync_file(&temp_file, &temp_path)?;
    drop(temp_file);
    rename(&temp_path, target_path)?;
    if let Some(observer) = observer {
        observer.file_renamed(&temp_path, target_path);
    }
    set_private_file_permissions(target_path)
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

fn read_optional_metadata(
    root: &Path,
    file_name: &str,
) -> Result<Option<String>, SecretStoreError> {
    let path = root.join(file_name);
    reject_symlink_path(&path)?;
    match fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(SecretStoreError::Filesystem { path, source }),
    }
}
