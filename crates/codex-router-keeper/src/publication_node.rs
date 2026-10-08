//! Captured symlink ownership; cleanup never removes a later owner's inode or target.
use crate::PublicationError;
use std::{fs, os::unix::fs::MetadataExt, path::PathBuf};
pub(crate) struct PublicationNode {
    pub(crate) path: PathBuf,
    target: PathBuf,
    device: u64,
    inode: u64,
}
impl PublicationNode {
    pub(crate) fn capture(path: PathBuf, target: PathBuf) -> Result<Self, PublicationError> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_symlink() || fs::read_link(&path)? != target {
            return Err(PublicationError::OwnershipLost);
        }
        Ok(Self {
            path,
            target,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    pub(crate) fn matches_current(&self) -> bool {
        fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_symlink()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
        }) && fs::read_link(&self.path).is_ok_and(|target| target == self.target)
    }
}
impl Drop for PublicationNode {
    fn drop(&mut self) {
        if self.matches_current() {
            let _cleanup = fs::remove_file(&self.path);
        }
    }
}
