use crate::ImageError;
use std::{
    fs::{DirBuilder, Metadata},
    os::unix::fs::{DirBuilderExt, MetadataExt},
    path::{Component, Path, PathBuf},
};
pub(crate) fn private_directory(path: &Path) -> Result<(), ImageError> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.is_dir()
        || meta.mode() & 0o777 != 0o700
        || meta.uid() != rustix::process::geteuid().as_raw()
    {
        return Err(ImageError::PrivateDirectory);
    }
    Ok(())
}
pub(crate) fn validate_root(root: &Path) -> Result<(), ImageError> {
    if !root.is_absolute() {
        return Err(ImageError::PrivateDirectory);
    }
    let mut path = PathBuf::new();
    for component in root.components() {
        if matches!(component, Component::ParentDir | Component::CurDir) {
            return Err(ImageError::PrivateDirectory);
        }
        path.push(component);
        if std::fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(ImageError::PrivateDirectory);
        }
    }
    private_directory(root)
}
pub(crate) fn ensure_private(path: &Path) -> Result<(), ImageError> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    private_directory(path)
}
pub(crate) struct OwnedImageNode {
    cleanup_path: Option<PathBuf>,
    pub device: u64,
    pub inode: u64,
}
impl OwnedImageNode {
    pub fn new(path: PathBuf, metadata: &Metadata) -> Self {
        Self {
            cleanup_path: Some(path),
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    pub fn observed_existing(metadata: &Metadata) -> Self {
        Self {
            cleanup_path: None,
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    pub fn disarm(mut self) {
        drop(self.cleanup_path.take());
    }
}
impl Drop for OwnedImageNode {
    fn drop(&mut self) {
        if let Some(path) = self.cleanup_path.as_ref()
            && let Ok(meta) = std::fs::symlink_metadata(path)
            && meta.is_file()
            && meta.dev() == self.device
            && meta.ino() == self.inode
        {
            let _cleanup = std::fs::remove_file(path);
        }
    }
}
