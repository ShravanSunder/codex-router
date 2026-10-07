//! Retain original listening file descriptions while child grants own only duplicates.
use crate::{ListenerAddress, RegistryError, SingletonAuthority, listener_address::AddressValue};
use codex_router_descriptor_boundary::{BoundaryError, DescriptorGate, OwnedListener, OwnedSocket};
use codex_router_keeper_protocol::{DescriptorSpec, ListenerKind};
use std::{
    fs::Permissions,
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    },
    path::PathBuf,
};

pub struct ListenerRegistry {
    entries: Vec<RegistryEntry>,
    operator: Option<RegistryEntry>,
    // Fields drop in declaration order: listeners close/clean before releasing authority.
    authority: SingletonAuthority,
}
struct RegistryEntry {
    kind: Option<ListenerKind>,
    requested: ListenerAddress,
    actual: ListenerAddress,
    listener: OwnedListener,
    _cleanup: Option<SocketNode>,
}
struct SocketNode {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl SocketNode {
    fn capture(path: PathBuf) -> Result<Self, RegistryError> {
        let metadata = std::fs::symlink_metadata(&path).map_err(RegistryError::Filesystem)?;
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    fn matches_current(&self) -> bool {
        std::fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.dev() == self.device && metadata.ino() == self.inode)
    }
}
impl Drop for SocketNode {
    fn drop(&mut self) {
        if self.matches_current() {
            let _cleanup = std::fs::remove_file(&self.path);
        }
    }
}
/// One descriptor and its registry-derived association, for a grant consumer.
pub struct GrantedListener {
    kind: ListenerKind,
    address: ListenerAddress,
    descriptor: OwnedFd,
}
impl GrantedListener {
    pub fn kind(&self) -> &ListenerKind {
        &self.kind
    }
    pub fn address(&self) -> &ListenerAddress {
        &self.address
    }
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }
    pub fn descriptor_spec(&self) -> DescriptorSpec {
        self.address.descriptor_spec()
    }
    pub fn into_parts(self) -> (ListenerKind, ListenerAddress, OwnedFd) {
        (self.kind, self.address, self.descriptor)
    }
}
impl RegistryEntry {
    async fn bind(
        kind: Option<ListenerKind>,
        requested: ListenerAddress,
    ) -> Result<Self, RegistryError> {
        let gate = DescriptorGate::global();
        let (listener, actual, cleanup) = match &requested.0 {
            AddressValue::Unix(path) => {
                // Revalidate the existing parent at the effect boundary; never unlink occupied paths.
                crate::listener_address::validate_private_path(path)?;
                let listener = OwnedListener::bind_unix(path, gate).await?;
                let cleanup = SocketNode::capture(path.clone())?;
                std::fs::set_permissions(path, Permissions::from_mode(0o600))
                    .map_err(RegistryError::Filesystem)?;
                (listener, requested.clone(), Some(cleanup))
            }
            AddressValue::Tcp(address) => {
                let listener = OwnedListener::bind_tcp(*address, gate).await?;
                let actual = ListenerAddress::tcp(listener.tcp_address()?)?;
                (listener, actual, None)
            }
        };
        Ok(Self {
            kind,
            requested,
            actual,
            listener,
            _cleanup: cleanup,
        })
    }
}
impl ListenerRegistry {
    pub fn new(authority: SingletonAuthority) -> Self {
        Self {
            entries: Vec::new(),
            operator: None,
            authority,
        }
    }
    pub async fn bind(
        &mut self,
        kind: ListenerKind,
        address: ListenerAddress,
    ) -> Result<ListenerAddress, RegistryError> {
        address.validate_kind(&kind)?;
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.kind.as_ref() == Some(&kind))
        {
            if entry.requested != address && entry.actual != address {
                return Err(RegistryError::AssociationConflict);
            }
            return Ok(entry.actual.clone());
        }
        if self
            .entries
            .iter()
            .chain(self.operator.iter())
            .any(|entry| entry.requested == address || entry.actual == address)
        {
            return Err(RegistryError::AssociationConflict);
        }
        let entry = RegistryEntry::bind(Some(kind), address).await?;
        let actual = entry.actual.clone();
        self.entries.push(entry);
        Ok(actual)
    }
    pub async fn duplicate(&self, kind: &ListenerKind) -> Result<GrantedListener, RegistryError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.kind.as_ref() == Some(kind))
            .ok_or(RegistryError::ListenerAbsent)?;
        Ok(GrantedListener {
            kind: kind.clone(),
            address: entry.actual.clone(),
            descriptor: DescriptorGate::global()
                .duplicate(entry.listener.as_fd())
                .await?,
        })
    }
    /// Only the acquired singleton's operator artifact may be reclaimed when stale.
    pub async fn bind_operator(&mut self, path: PathBuf) -> Result<(), RegistryError> {
        if path.file_name() != Some(std::ffi::OsStr::new("host.sock"))
            || path.parent() != self.authority.lock_path().parent()
        {
            return Err(RegistryError::OperatorPathMismatch);
        }
        let address = ListenerAddress::unix(path.clone())?;
        if let Some(operator) = &self.operator {
            return if operator.actual == address {
                Ok(())
            } else {
                Err(RegistryError::AssociationConflict)
            };
        }
        if self.entries.iter().any(|entry| entry.actual == address) {
            return Err(RegistryError::AssociationConflict);
        }
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_socket() {
                    return Err(RegistryError::OperatorOccupied);
                }
                let identity = (metadata.dev(), metadata.ino());
                // Connect-and-drop only; running keeper does not receive Unix bytes.
                let probe = tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    OwnedSocket::connect_unix(&path, DescriptorGate::global()),
                )
                .await;
                let stale = matches!(probe, Ok(Err(BoundaryError::Io(ref error))) if error.kind() == std::io::ErrorKind::ConnectionRefused);
                if !stale
                    || !std::fs::symlink_metadata(&path)
                        .is_ok_and(|current| (current.dev(), current.ino()) == identity)
                {
                    return Err(RegistryError::OperatorOccupied);
                }
                // Remove only the unchanged stale artifact after exclusive authority was acquired.
                std::fs::remove_file(&path).map_err(RegistryError::Filesystem)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(RegistryError::Filesystem(error)),
        }
        self.operator = Some(RegistryEntry::bind(None, address).await?);
        Ok(())
    }
    pub fn operator_listener(&self) -> Result<&OwnedListener, RegistryError> {
        self.operator
            .as_ref()
            .map(|entry| &entry.listener)
            .ok_or(RegistryError::ListenerAbsent)
    }
}
