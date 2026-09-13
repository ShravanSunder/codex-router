//! Private socket binding and inode-scoped cleanup shared by local protocol listeners.
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;
pub(crate) struct PrivateSocketListener {
    pub(crate) listener: UnixListener,
    socket_path: PathBuf,
    socket_device: u64,
    socket_inode: u64,
}
impl PrivateSocketListener {
    /// The caller creates a dedicated private directory; existing paths are never unlinked.
    pub(crate) fn bind(socket_path: &Path) -> io::Result<Self> {
        if !socket_path.is_absolute() {
            return Err(io::Error::other("Control socket must be absolute"));
        }
        let parent = socket_path
            .parent()
            .ok_or_else(|| io::Error::other("missing socket parent"))?;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other("Control directory must be private"));
        }
        let listener = UnixListener::bind(socket_path)?;
        let metadata = std::fs::symlink_metadata(socket_path)?;
        let service = Self {
            listener,
            socket_path: socket_path.to_owned(),
            socket_device: metadata.dev(),
            socket_inode: metadata.ino(),
        };
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
        Ok(service)
    }
}
impl Drop for PrivateSocketListener {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.socket_path)
            && metadata.dev() == self.socket_device
            && metadata.ino() == self.socket_inode
        {
            let _cleanup = std::fs::remove_file(&self.socket_path);
        }
    }
}
