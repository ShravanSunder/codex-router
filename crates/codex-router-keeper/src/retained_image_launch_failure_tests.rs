use super::*;
use crate::image_warmup::WarmupCleanup;
use crate::owned_process_group::{PostSpawnCheckpoint, launch_failure_tests::cleanup_fences};
use crate::{GroupStopError, ImageLaunchOutcome};
fn compiled_fixture() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = PathBuf::from(
        std::env::var_os("KEEPER_LAUNCH_FAILURE_FIXTURE")
            .ok_or("explicit built fixture path required")?,
    );
    if !path.is_absolute() || !path.is_file() {
        return Err("compiled fixture must be an existing absolute path".into());
    }
    Ok(path)
}
#[tokio::test]
async fn failed_retained_launch_owns_image_until_same_cleanup_fence() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let source = compiled_fixture()?;
    let lease = registry.pin(&source, &expected_build(0x11)?).await?;
    let record = lease.image().clone();
    let marker = root.join("retained-join.json");
    let outcome = registry
        .spawn_with_checkpoint(
            &lease,
            &[],
            PostSpawnCheckpoint::GroupChangeMarker(marker.clone()),
        )
        .await?;
    let ImageLaunchOutcome::CleanupPending {
        reason: GroupStopError::GroupMismatch,
        mut cleanup,
        image,
    } = outcome
    else {
        return Err("retained-image setup mismatch lost owning outcome".into());
    };
    drop(lease);
    if image.image() != &record
        || !registry.collect().await?.is_empty()
        || !record.retained_path().exists()
    {
        return Err("pending launch debt did not retain the exact image".into());
    }
    cleanup_fences(&mut cleanup, &marker, rustix::process::getpgrp()).await?;
    if !registry.collect().await?.is_empty() {
        return Err("completed-but-held image lease was collected".into());
    }
    eprintln!(
        "RETAINED_LAUNCH_IMAGE path={:?} device={} inode={} cleanup_pid={:?}",
        record.retained_path(),
        record.device(),
        record.inode(),
        cleanup.leader_pid()
    );
    drop(image);
    if registry.collect().await? != vec![record.clone()] || record.retained_path().exists() {
        return Err("unreferenced completed launch image was not collected".into());
    }
    Ok(())
}
#[tokio::test]
async fn failed_warmup_transfers_original_cleanup_and_inode_to_registry() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let source = compiled_fixture()?;
    let mut registry = ImageRegistry::new(&root).await?;
    let marker = root.join("warmup-join.json");
    let captured = capture(&source, true).await?;
    let path = registry
        .images_root
        .join(digest_hex(&captured.digest))
        .join(".candidate");
    let error = registry
        .pin_with_checkpoint(
            &source,
            &expected_build(0x11)?,
            exdev,
            PREPARE_DEADLINE,
            PostSpawnCheckpoint::GroupChangeMarker(marker.clone()),
        )
        .await;
    if !matches!(
        error,
        Err(ImageError::Process(GroupStopError::GroupMismatch))
    ) || registry.pending_warmups.len() != 1
        || !registry.entries.is_empty()
    {
        return Err("warmup setup failure did not transfer cleanup debt".into());
    }
    let pending = registry
        .pending_warmups
        .first_mut()
        .ok_or("pending image owner absent")?;
    let before = std::fs::metadata(&path)?;
    if before.dev() != pending._image.device || before.ino() != pending._image.inode {
        return Err("pending candidate inode identity lost".into());
    }
    let WarmupCleanup::FailedLaunch(cleanup) = &mut pending.cleanup else {
        return Err("wrong cleanup phase".into());
    };
    cleanup_fences(cleanup, &marker, rustix::process::getpgrp()).await?;
    if !path.exists() {
        return Err("warmup image released before registry observed fence".into());
    }
    registry.collect().await?;
    if path.exists() || !registry.pending_warmups.is_empty() {
        return Err("warmup image not released after matching cleanup fence".into());
    }
    Ok(())
}
