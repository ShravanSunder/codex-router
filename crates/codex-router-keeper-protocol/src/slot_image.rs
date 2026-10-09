//! Captured structure only; filesystem validation and launch authority belong to keeper.
use serde::{Deserialize, Serialize};
use std::{
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "SlotImageWire", into = "SlotImageWire")]
pub struct SlotImage {
    retained_path: PathBuf,
    file_sha256: [u8; 32],
    device: u64,
    inode: u64,
}
#[derive(Debug, thiserror::Error)]
pub enum SlotImageError {
    #[error(
        "retained image path must be absolute and name a file without dot, empty or NUL components"
    )]
    Path,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlotImageWire {
    retained_path: PathBuf,
    file_sha256: [u8; 32],
    device: u64,
    inode: u64,
}
impl SlotImage {
    pub fn new(
        retained_path: PathBuf,
        file_sha256: [u8; 32],
        device: u64,
        inode: u64,
    ) -> Result<Self, SlotImageError> {
        let bytes = retained_path.as_os_str().as_bytes();
        if !retained_path.is_absolute()
            || retained_path.file_name().is_none()
            || bytes.contains(&0)
            || bytes
                .split(|b| *b == b'/')
                .skip(1)
                .any(|part| part.is_empty() || part == b"." || part == b"..")
        {
            return Err(SlotImageError::Path);
        }
        Ok(Self {
            retained_path,
            file_sha256,
            device,
            inode,
        })
    }
    pub fn retained_path(&self) -> &Path {
        &self.retained_path
    }
    pub fn file_sha256(&self) -> &[u8; 32] {
        &self.file_sha256
    }
    pub fn device(&self) -> u64 {
        self.device
    }
    pub fn inode(&self) -> u64 {
        self.inode
    }
}
impl TryFrom<SlotImageWire> for SlotImage {
    type Error = SlotImageError;
    fn try_from(value: SlotImageWire) -> Result<Self, Self::Error> {
        Self::new(
            value.retained_path,
            value.file_sha256,
            value.device,
            value.inode,
        )
    }
}
impl From<SlotImage> for SlotImageWire {
    fn from(value: SlotImage) -> Self {
        Self {
            retained_path: value.retained_path,
            file_sha256: value.file_sha256,
            device: value.device,
            inode: value.inode,
        }
    }
}
