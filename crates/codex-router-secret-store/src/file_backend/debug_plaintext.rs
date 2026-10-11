//! Explicit root-local debug plaintext policy and its guarded file operations.

use super::*;
#[cfg(debug_assertions)]
use crate::credential_key::AccountCredentialKey;
use crate::credential_store_lock::CredentialStoreLock;
#[cfg(debug_assertions)]
use crate::credential_store_lock::CredentialStoreLockMode;

const DECLARATION_FILE: &str = "debug-plaintext.marker";
const DECLARATION_VALUE: &[u8] = b"debug-plaintext-v1\n";

impl FileSecretStore {
    /// Refuses a declared plaintext root before encrypted key retrieval, in every build.
    pub fn reject_debug_plaintext_declaration(root: &Path) -> Result<(), SecretStoreError> {
        if read_declaration(root)? {
            return Err(SecretStoreError::DebugPlaintextUnavailable);
        }
        Ok(())
    }

    /// Reopens an explicitly declared root; release builds always refuse it.
    pub fn open_declared_debug_plaintext(root: &Path) -> Result<Option<Self>, SecretStoreError> {
        if !read_declaration(root)? {
            return Ok(None);
        }
        #[cfg(not(debug_assertions))]
        return Err(SecretStoreError::DebugPlaintextUnavailable);
        #[cfg(debug_assertions)]
        {
            validate_debug_root(root)?;
            let mut store = Self::open(root)?;
            let _lock =
                CredentialStoreLock::acquire(store.root(), CredentialStoreLockMode::Exclusive)?;
            validate_plaintext_layout(store.root(), true)?;
            if !read_declaration(store.root())? {
                return Err(SecretStoreError::DebugPlaintextUnavailable);
            }
            store.credential_policy = FileCredentialPolicy::DebugPlaintextPooled;
            Ok(Some(store))
        }
    }

    /// Explicitly initializes actual plaintext pooled storage in a dedicated debug root.
    #[cfg(debug_assertions)]
    pub fn initialize_debug_plaintext(root: &Path) -> Result<Self, SecretStoreError> {
        validate_debug_root(root)?;
        // A pooled temp may still belong to an active Shared-lock writer.
        // Classify it as abandoned only after the Exclusive lock is acquired.
        validate_plaintext_layout(root, false)?;
        let mut store = Self::open(root)?;
        let _lock = CredentialStoreLock::acquire(store.root(), CredentialStoreLockMode::Exclusive)?;
        validate_plaintext_layout(store.root(), true)?;
        if !read_declaration(store.root())? {
            store.write_atomically(
                &store.root.join(DECLARATION_FILE),
                DECLARATION_FILE,
                DECLARATION_VALUE,
            )?;
        }
        store.credential_policy = FileCredentialPolicy::DebugPlaintextPooled;
        Ok(store)
    }

    pub(super) fn authorize_plaintext_credential_operation(
        &self,
        key: &SecretKey,
    ) -> Result<Option<CredentialStoreLock>, SecretStoreError> {
        if !has_credential_bundle_marker(key) {
            return Ok(None);
        }
        #[cfg(debug_assertions)]
        if self.credential_policy == FileCredentialPolicy::DebugPlaintextPooled {
            AccountCredentialKey::parse(key)?.ok_or_else(|| {
                SecretStoreError::InvalidCredentialKey {
                    key: key.as_str().to_owned(),
                }
            })?;
            let lock = CredentialStoreLock::acquire(self.root(), CredentialStoreLockMode::Shared)?;
            validate_debug_root(self.root())?;
            validate_plaintext_layout(self.root(), false)?;
            if !read_declaration(self.root())? {
                return Err(SecretStoreError::DebugPlaintextUnavailable);
            }
            return Ok(Some(lock));
        }
        reject_pooled_credential_key(key)?;
        Ok(None)
    }

    pub(super) fn delete_plaintext_staged_credential(
        &self,
        key: &SecretKey,
    ) -> Result<(), SecretStoreError> {
        #[cfg(debug_assertions)]
        if self.credential_policy == FileCredentialPolicy::DebugPlaintextPooled {
            AccountCredentialKey::parse(key)?.ok_or_else(|| {
                SecretStoreError::InvalidCredentialKey {
                    key: key.as_str().to_owned(),
                }
            })?;
            let _lock = self.authorize_plaintext_credential_operation(key)?;
            let path = self.secret_path(key);
            reject_symlink_path(&path)?;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(source) => {
                    return Err(SecretStoreError::Filesystem { path, source });
                }
            }
            self.record_file_removed(&path);
            return Ok(());
        }
        Err(SecretStoreError::PooledCredentialRequiresEncryption {
            key: key.as_str().to_owned(),
        })
    }

    pub(super) fn prune_plaintext_generations(
        &self,
        provider: codex_router_core::provider::Provider,
        account_id: &codex_router_core::ids::AccountId,
        previously_active_generation: u64,
    ) -> Result<Vec<u64>, SecretStoreError> {
        #[cfg(debug_assertions)]
        if self.credential_policy == FileCredentialPolicy::DebugPlaintextPooled {
            let scope_key =
                AccountCredentialKey::new(provider, account_id.clone(), 1)?.secret_key()?;
            let _lock = self.authorize_plaintext_credential_operation(&scope_key)?;
            let mut removed = Vec::new();
            for secret_key in self.list_pooled_credential_files("secret")? {
                let key = AccountCredentialKey::parse(&secret_key)?.ok_or_else(|| {
                    SecretStoreError::InvalidCredentialKey {
                        key: secret_key.as_str().to_owned(),
                    }
                })?;
                if key.provider() == provider
                    && key.account_id() == account_id
                    && key.generation() < previously_active_generation
                {
                    let path = self.secret_path(&secret_key);
                    reject_symlink_path(&path)?;
                    match fs::remove_file(&path) {
                        Ok(()) => {
                            self.record_file_removed(&path);
                            removed.push(key.generation());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(source) => return Err(SecretStoreError::Filesystem { path, source }),
                    }
                }
            }
            return Ok(removed);
        }
        let _ = (provider, account_id, previously_active_generation);
        Ok(Vec::new())
    }
}

fn read_declaration(root: &Path) -> Result<bool, SecretStoreError> {
    reject_symlink_path(root)?;
    validate_existing_parent(root)?;
    let path = root.join(DECLARATION_FILE);
    reject_symlink_path(&path)?;
    match fs::read(&path) {
        Ok(value) if value == DECLARATION_VALUE => Ok(true),
        Ok(_) => Err(SecretStoreError::InvalidDebugPlaintextDeclaration),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(SecretStoreError::Filesystem { path, source }),
    }
}

#[cfg(debug_assertions)]
fn validate_plaintext_layout(
    root: &Path,
    reject_pooled_temps: bool,
) -> Result<(), SecretStoreError> {
    read_declaration(root)?;
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SecretStoreError::Filesystem {
                path: root.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| SecretStoreError::Filesystem {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let encrypted_metadata_temp =
            name.starts_with(".store-id.tmp.") || name.starts_with(".format-v2.marker.tmp.");
        let pooled_temp =
            name.starts_with('.') && name.contains("_credential_bundle.") && name.contains(".tmp.");
        // Initialization/reopening holds the exclusive store lock, so any
        // remaining pooled temp has unknown prior ownership. Active plaintext
        // writes share the lock and may legitimately have each other's temps.
        if name == "store-id"
            || name == "format-v2.marker"
            || name.ends_with(".v2")
            || encrypted_metadata_temp
            || (reject_pooled_temps && pooled_temp)
        {
            return Err(SecretStoreError::DebugPlaintextRootRejected);
        }
        if let Some(key_name) = name.strip_suffix(".secret") {
            let key = SecretKey::new(key_name)?;
            if has_credential_bundle_marker(&key) {
                AccountCredentialKey::parse(&key)?.ok_or_else(|| {
                    SecretStoreError::InvalidCredentialKey {
                        key: key_name.to_owned(),
                    }
                })?;
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
            }
        }
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn validate_debug_root(root: &Path) -> Result<(), SecretStoreError> {
    reject_codex_home_path(root)?;
    reject_symlink_path(root)?;
    validate_existing_parent(root)?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or(SecretStoreError::DebugPlaintextRootRejected)?;
    let root = resolved_root(root)?;
    let protected = resolved_root(&home.join(".codex-router"))?;
    if root.starts_with(&protected)
        || protected.starts_with(&root)
        || root == resolved_root(&std::env::temp_dir())?
        || root == resolved_root(Path::new("/tmp"))?
    {
        return Err(SecretStoreError::DebugPlaintextRootRejected);
    }
    Ok(())
}

#[cfg(debug_assertions)]
fn resolved_root(path: &Path) -> Result<PathBuf, SecretStoreError> {
    if !path.is_absolute() {
        return Err(SecretStoreError::DebugPlaintextRootRejected);
    }
    let mut ancestor = path.to_path_buf();
    let mut tail = Vec::new();
    let mut resolved = loop {
        match fs::canonicalize(&ancestor) {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tail.push(
                    ancestor
                        .file_name()
                        .ok_or(SecretStoreError::DebugPlaintextRootRejected)?
                        .to_os_string(),
                );
                if !ancestor.pop() {
                    return Err(SecretStoreError::DebugPlaintextRootRejected);
                }
            }
            Err(source) => {
                return Err(SecretStoreError::Filesystem {
                    path: ancestor,
                    source,
                });
            }
        }
    };
    for part in tail.into_iter().rev() {
        if part == ".." {
            resolved.pop();
        } else if part != "." {
            resolved.push(part);
        }
    }
    Ok(resolved)
}

#[cfg(test)]
#[path = "debug_plaintext_tests.rs"]
mod tests;
