use crate::ImageError;
use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper_protocol::SlotImage;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
pub(crate) struct CapturedImage {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
    pub metadata: Metadata,
}
pub(crate) fn executable_metadata(metadata: &Metadata) -> Result<(), ImageError> {
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 || metadata.len() == 0 {
        return Err(ImageError::InvalidExecutable);
    }
    Ok(())
}
pub(crate) fn same_node(metadata: &Metadata, image: &SlotImage) -> bool {
    metadata.is_file() && metadata.dev() == image.device() && metadata.ino() == image.inode()
}
fn stable_metadata(before: &Metadata, after: &Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mode() == after.mode()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}
pub(crate) async fn capture(
    path: &Path,
    resolve_symlink: bool,
) -> Result<CapturedImage, ImageError> {
    let path = path.to_owned();
    let path = tokio::task::spawn_blocking(move || {
        if resolve_symlink {
            std::fs::canonicalize(path)
        } else {
            Ok(path)
        }
    })
    .await??;
    let open_path = path.clone();
    let creation = DescriptorGate::global().creation().await;
    let file = tokio::task::spawn_blocking(move || {
        OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(open_path)
    })
    .await??;
    drop(creation);
    tokio::task::spawn_blocking(move || measure(file, path)).await?
}
fn measure(mut file: File, path: PathBuf) -> Result<CapturedImage, ImageError> {
    let before = file.metadata()?;
    executable_metadata(&before)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let at_path = std::fs::symlink_metadata(&path)?;
    if !stable_metadata(&before, &after)
        || at_path.dev() != after.dev()
        || at_path.ino() != after.ino()
        || !at_path.is_file()
    {
        return Err(ImageError::ImageUnavailable);
    }
    let digest = Sha256::digest(&bytes).into();
    Ok(CapturedImage {
        path,
        bytes,
        digest,
        metadata: after,
    })
}
pub(crate) async fn verify(image: &SlotImage) -> Result<(), ImageError> {
    let actual = capture(image.retained_path(), false).await?;
    if !same_node(&actual.metadata, image) || &actual.digest != image.file_sha256() {
        return Err(ImageError::ImageUnavailable);
    }
    Ok(())
}
pub(crate) fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
