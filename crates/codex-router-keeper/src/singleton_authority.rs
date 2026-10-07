//! Nonblocking acquisition of the existing lock artifact before endpoint effects.
use crate::{RegistryError, listener_address::validate_private_path};
use codex_router_descriptor_boundary::DescriptorGate;
use std::{
    fs::{File, OpenOptions, Permissions, TryLockError},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};
pub struct SingletonAuthority {
    lock_file: File,
    lock_path: PathBuf,
}
impl SingletonAuthority {
    pub async fn acquire(lock_path: &Path) -> Result<Self, RegistryError> {
        validate_private_path(lock_path)?;
        let _shared = DescriptorGate::global().creation().await;
        let lock_file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(lock_path)
            .map_err(RegistryError::Singleton)?;
        match lock_file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(RegistryError::AlreadyRunning),
            Err(TryLockError::Error(error)) => return Err(RegistryError::Singleton(error)),
        }
        lock_file
            .set_permissions(Permissions::from_mode(0o600))
            .map_err(RegistryError::Singleton)?;
        Ok(Self {
            lock_file,
            lock_path: lock_path.to_owned(),
        })
    }
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.lock_file.as_fd()
    }
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}
