#![cfg(test)]

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::file_backend::FileSecretStore;
use crate::keychain_data_key::KeychainAccess;
use crate::keychain_data_key::KeychainAccessError;
use crate::keychain_data_key::PooledCredentialDataKey;
use crate::keychain_data_key::load_or_create_pooled_credential_data_key;
use crate::model::SecretStoreError;

#[derive(Default)]
pub(crate) struct RecordingKeychainAccess {
    state: Mutex<KeychainState>,
}

#[derive(Default)]
struct KeychainState {
    items: HashMap<(String, String), Vec<u8>>,
    allow_fixture_adds: bool,
    allow_noninteractive_reads: bool,
    interactive_read_count: usize,
    noninteractive_read_count: usize,
    add_attempt_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct KeychainCallCounts {
    pub(crate) interactive_reads: usize,
    pub(crate) noninteractive_reads: usize,
    pub(crate) add_attempts: usize,
}

impl RecordingKeychainAccess {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn allow_fixture_adds(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.allow_fixture_adds = true;
        state.allow_noninteractive_reads = true;
    }

    pub(crate) fn stop_fixture_adds_and_reset_counts(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.allow_fixture_adds = false;
        state.interactive_read_count = 0;
        state.noninteractive_read_count = 0;
        state.add_attempt_count = 0;
    }

    pub(crate) fn set_noninteractive_reads_available(&self, available: bool) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .allow_noninteractive_reads = available;
    }

    pub(crate) fn remove_all_items(&self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .items
            .clear();
    }

    pub(crate) fn replace_all_item_bytes(&self, secret: Vec<u8>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for stored_secret in state.items.values_mut() {
            *stored_secret = secret.clone();
        }
    }

    pub(crate) fn call_counts(&self) -> KeychainCallCounts {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        KeychainCallCounts {
            interactive_reads: state.interactive_read_count,
            noninteractive_reads: state.noninteractive_read_count,
            add_attempts: state.add_attempt_count,
        }
    }
}

impl KeychainAccess for RecordingKeychainAccess {
    fn read_secret(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.interactive_read_count += 1;
        Ok(state
            .items
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn read_secret_without_user_interaction(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainAccessError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.noninteractive_read_count += 1;
        if !state.allow_noninteractive_reads {
            return Err(KeychainAccessError::Unavailable);
        }
        Ok(state
            .items
            .get(&(service.to_owned(), account.to_owned()))
            .cloned())
    }

    fn add_secret(
        &self,
        service: &str,
        account: &str,
        secret: &[u8],
    ) -> Result<(), KeychainAccessError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.add_attempt_count += 1;
        if !state.allow_fixture_adds {
            return Err(KeychainAccessError::Unavailable);
        }
        let item_key = (service.to_owned(), account.to_owned());
        if state.items.contains_key(&item_key) {
            return Err(KeychainAccessError::Unavailable);
        }
        state.items.insert(item_key, secret.to_vec());
        Ok(())
    }
}

pub(crate) fn seed_fresh_data_key(
    secret_root: &Path,
    keychain: &RecordingKeychainAccess,
) -> Result<PooledCredentialDataKey, SecretStoreError> {
    let file_store = FileSecretStore::open(secret_root)?;
    keychain.allow_fixture_adds();
    let data_key = load_or_create_pooled_credential_data_key(&file_store, keychain)?;
    keychain.stop_fixture_adds_and_reset_counts();
    Ok(data_key)
}

pub(crate) fn seed_fresh_ready_store(
    secret_root: &Path,
    keychain: &RecordingKeychainAccess,
) -> Result<PooledCredentialDataKey, SecretStoreError> {
    let file_store = FileSecretStore::open(secret_root)?;
    keychain.allow_fixture_adds();
    let data_key = load_or_create_pooled_credential_data_key(&file_store, keychain)?;
    file_store.write_format_v2_marker()?;
    keychain.stop_fixture_adds_and_reset_counts();
    Ok(data_key)
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct SnapshotEntry {
    relative_path: PathBuf,
    kind: SnapshotEntryKind,
    mode: u32,
    bytes: Vec<u8>,
    link_target: Option<PathBuf>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SnapshotEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

pub(crate) fn assert_root_unchanged(root: &Path, before: &[SnapshotEntry]) {
    let after = snapshot_root(root);
    assert!(
        before == after,
        "existing-only operation changed secret-root entries, modes, or bytes"
    );
}

pub(crate) fn snapshot_root(root: &Path) -> Vec<SnapshotEntry> {
    let mut entries = Vec::new();
    if root.exists() {
        snapshot_entry(root, root, &mut entries);
    }
    entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    entries
}

fn snapshot_entry(root: &Path, path: &Path, entries: &mut Vec<SnapshotEntry>) {
    let metadata =
        fs::symlink_metadata(path).unwrap_or_else(|_| panic!("snapshot metadata read failed"));
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        SnapshotEntryKind::Symlink
    } else if file_type.is_dir() {
        SnapshotEntryKind::Directory
    } else if file_type.is_file() {
        SnapshotEntryKind::File
    } else {
        SnapshotEntryKind::Other
    };
    let bytes = if kind == SnapshotEntryKind::File {
        fs::read(path).unwrap_or_else(|_| panic!("snapshot file read failed"))
    } else {
        Vec::new()
    };
    let link_target = if kind == SnapshotEntryKind::Symlink {
        Some(fs::read_link(path).unwrap_or_else(|_| panic!("snapshot symlink read failed")))
    } else {
        None
    };
    let relative_path = path
        .strip_prefix(root)
        .unwrap_or_else(|_| panic!("snapshot path escaped root"))
        .to_path_buf();
    entries.push(SnapshotEntry {
        relative_path,
        kind,
        mode: file_mode(&metadata),
        bytes,
        link_target,
    });
    if kind == SnapshotEntryKind::Directory {
        let mut children = fs::read_dir(path)
            .unwrap_or_else(|_| panic!("snapshot directory read failed"))
            .map(|entry| {
                entry
                    .unwrap_or_else(|_| panic!("snapshot entry read failed"))
                    .path()
            })
            .collect::<Vec<_>>();
        children.sort();
        for child in children {
            snapshot_entry(root, &child, entries);
        }
    }
}

#[cfg(unix)]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn file_mode(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}
