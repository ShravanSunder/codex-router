//! Recognize immediate cache nodes without granting deletion or BuildInfo authority.
use crate::{
    ImageError,
    image_directory::{private_directory, validate_root},
    image_identity::capture,
};
use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper_protocol::SlotImage;
use std::{
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

async fn child_paths(directory: &Path) -> Result<Vec<PathBuf>, ImageError> {
    let directory = directory.to_owned();
    let creation = DescriptorGate::global().creation().await;
    let result = tokio::task::spawn_blocking(move || {
        std::fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, std::io::Error>>()
    })
    .await?;
    drop(creation);
    Ok(result?)
}

pub(super) async fn validate_layout(directory: &Path) -> Result<(), ImageError> {
    let path = directory.to_owned();
    tokio::task::spawn_blocking(move || {
        validate_root(&path)?;
        private_directory(path.parent().ok_or(ImageError::PrivateDirectory)?)?;
        let file = std::fs::symlink_metadata(path.join("codex-router"))?;
        if !file.is_file() || file.uid() != rustix::process::geteuid().as_raw() {
            return Err(ImageError::ForeignNode);
        }
        Ok(())
    })
    .await??;
    let children = child_paths(directory).await?;
    let [child] = children.as_slice() else {
        return Err(ImageError::ForeignNode);
    };
    if child != &directory.join("codex-router") {
        return Err(ImageError::ForeignNode);
    }
    Ok(())
}

fn canonical_digest(path: &Path) -> Option<[u8; 32]> {
    let name = path.file_name()?.to_str()?;
    if name.len() != 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0; 32];
    for (output, pair) in digest.iter_mut().zip(name.as_bytes().as_chunks::<2>().0) {
        let pair = std::str::from_utf8(pair).ok()?;
        *output = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

pub(super) async fn discover(root: &Path) -> Result<Vec<SlotImage>, ImageError> {
    let check_root = root.to_owned();
    tokio::task::spawn_blocking(move || {
        validate_root(&check_root)?;
        private_directory(check_root.parent().ok_or(ImageError::PrivateDirectory)?)
    })
    .await??;
    let mut records = Vec::new();
    for directory in child_paths(root).await? {
        let Some(digest) = canonical_digest(&directory) else {
            continue;
        };
        // Unknown, malformed, non-private or foreign layouts remain untouched.
        if validate_layout(&directory).await.is_err() {
            continue;
        }
        let Ok(actual) = capture(&directory.join("codex-router"), false).await else {
            continue;
        };
        if actual.digest != digest || actual.metadata.uid() != rustix::process::geteuid().as_raw() {
            continue;
        }
        records.push(SlotImage::new(
            actual.path,
            digest,
            actual.metadata.dev(),
            actual.metadata.ino(),
        )?);
    }
    Ok(records)
}
