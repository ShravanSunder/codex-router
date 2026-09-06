//! Atomic, inode-owned service manifest publication after listeners are bound.
use crate::new_service_uuid;
use communication_protocol::ServiceManifest;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub struct ManifestPublication {
    path: PathBuf,
    device: u64,
    inode: u64,
}
impl ManifestPublication {
    /// Caller holds service ownership and has bound the advertised listener.
    pub fn publish(directory: &Path, manifest: &ServiceManifest) -> io::Result<Self> {
        if manifest.version != 1 {
            return Err(io::Error::other("unsupported manifest version"));
        }
        let path = directory.join("service.json");
        let suffix: String = new_service_uuid()?.into();
        let temporary = directory.join(format!(".service-manifest-{suffix}.tmp"));
        let result = (|| {
            let bytes = serde_json::to_vec(manifest).map_err(io::Error::other)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            let metadata = file.metadata()?;
            let publication = Self {
                path: path.clone(),
                device: metadata.dev(),
                inode: metadata.ino(),
            };
            std::fs::rename(&temporary, &path)?;
            File::open(directory)?.sync_all()?;
            Ok(publication)
        })();
        if result.is_err() {
            let _cleanup = std::fs::remove_file(temporary);
        }
        result
    }
}
impl Drop for ManifestPublication {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
        {
            let _cleanup = std::fs::remove_file(&self.path);
        }
    }
}
