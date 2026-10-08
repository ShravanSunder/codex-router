//! Validated local addresses, separate from business configuration and wire schemas.
use crate::RegistryError;
use codex_router_keeper_protocol::{DescriptorSpec, ListenerKind};
use std::{
    net::SocketAddr,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListenerAddress(pub(crate) AddressValue);
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AddressValue {
    Unix(PathBuf),
    Tcp(SocketAddr),
}
pub(crate) fn validate_private_path(path: &Path) -> Result<(), RegistryError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(RegistryError::PrivatePathRequired);
    }
    let parent = path.parent().ok_or(RegistryError::PrivatePathRequired)?;
    let metadata = std::fs::symlink_metadata(parent).map_err(RegistryError::Filesystem)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(RegistryError::PrivatePathRequired);
    }
    Ok(())
}
impl ListenerAddress {
    pub fn unix(path: PathBuf) -> Result<Self, RegistryError> {
        validate_private_path(&path)?;
        Ok(Self(AddressValue::Unix(path)))
    }
    pub fn tcp(address: SocketAddr) -> Result<Self, RegistryError> {
        if !address.ip().is_loopback() {
            return Err(RegistryError::LoopbackRequired);
        }
        Ok(Self(AddressValue::Tcp(address)))
    }
    pub fn descriptor_spec(&self) -> DescriptorSpec {
        match &self.0 {
            AddressValue::Unix(path) => DescriptorSpec::UnixListener { path: path.clone() },
            AddressValue::Tcp(address) => DescriptorSpec::TcpListener { address: *address },
        }
    }
    pub(crate) fn validate_kind(&self, kind: &ListenerKind) -> Result<(), RegistryError> {
        let matches = match (kind, &self.0) {
            (ListenerKind::McpHttp | ListenerKind::ProxyHttp, AddressValue::Tcp(_)) => true,
            (ListenerKind::RouterSessionFace { endpoint }, AddressValue::Unix(path)) => {
                let endpoint: String = endpoint.clone().into();
                path.parent().and_then(Path::file_name)
                    == Some(std::ffi::OsStr::new("router-sessions"))
                    && path.file_name().is_some_and(|name| {
                        name == std::ffi::OsStr::new(&format!("{endpoint}.sock"))
                    })
            }
            (
                ListenerKind::CollaborationControl
                | ListenerKind::NativeRelay
                | ListenerKind::AcpChannel
                | ListenerKind::ProviderLink,
                AddressValue::Unix(_),
            ) => true,
            _ => false,
        };
        if matches {
            Ok(())
        } else {
            Err(RegistryError::KindAddressMismatch)
        }
    }
}
