use super::*;

async fn fresh_repin(link: LinkOperation) -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "restart", "valid", "RESTART_IMAGE", 1)?;
    let bytes = std::fs::read(&source)?;
    let mut first = ImageRegistry::new(&root).await?;
    let lease = first
        .pin_with_link(&source, &expected_build(1)?, link, PREPARE_DEADLINE)
        .await?;
    let record = lease.image().clone();
    let before = std::fs::metadata(record.retained_path())?;
    drop(lease);
    drop(first);
    if proof.exists() {
        std::fs::remove_file(&proof)?;
    }
    let mut fresh = ImageRegistry::new(&root).await?;
    let lease = fresh
        .pin_with_link(&source, &expected_build(1)?, link, PREPARE_DEADLINE)
        .await?;
    let after = std::fs::metadata(lease.image().retained_path())?;
    if lease.image() != &record
        || before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.mode() != after.mode()
        || std::fs::read(lease.image().retained_path())? != bytes
        || !proof.exists()
    {
        return Err("fresh registry changed image or omitted full warmup".into());
    }
    let pid =
        codex_router_keeper_protocol::ChildPid::new(std::fs::read_to_string(&proof)?.parse()?)?;
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
    {
        return Err("fresh re-pin warmup owner did not reap/empty".into());
    }
    eprintln!(
        "FRESH_REPIN inode={} warmup_pid={pid:?} exit0 process=ESRCH group=ESRCH",
        after.ino()
    );
    std::fs::remove_file(source)?;
    run_marker(&fresh, &lease, "RESTART_IMAGE").await?;
    if !fresh.collect().await?.is_empty() {
        return Err("fresh live lease collected".into());
    }
    drop(lease);
    if fresh.collect().await? != vec![record] {
        return Err("fresh released image not collected".into());
    }
    Ok(())
}

#[tokio::test]
async fn fresh_registry_repins_prior_hardlink_with_full_warmup() -> TestResult {
    fresh_repin(hard_link).await
}

#[tokio::test]
async fn fresh_registry_repins_prior_copy_with_full_warmup() -> TestResult {
    fresh_repin(exdev).await
}

#[tokio::test]
async fn startup_collection_discovers_only_orphans_after_references_rebuilt() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (live_source, _) = fixture(&root, "live", "valid", "LIVE_IMAGE", 1)?;
    let (old_source, _) = fixture(&root, "orphan", "valid", "OLD_IMAGE", 2)?;
    let mut first = ImageRegistry::new(&root).await?;
    let live = first.pin(&live_source, &expected_build(1)?).await?;
    let orphan = first.pin(&old_source, &expected_build(2)?).await?;
    let live_record = live.image().clone();
    let orphan_record = orphan.image().clone();
    drop(live);
    drop(orphan);
    drop(first);
    let mut fresh = ImageRegistry::new(&root).await?;
    if !orphan_record.retained_path().exists() || !live_record.retained_path().exists() {
        return Err("constructor eagerly deleted disk images".into());
    }
    let live = fresh
        .reacquire(live_record.clone(), expected_build(1)?)
        .await?;
    let cloned_reference = live.clone();
    drop(live);
    let removed = fresh.collect().await?;
    if removed != vec![orphan_record.clone()]
        || orphan_record.retained_path().exists()
        || !live_record.retained_path().exists()
    {
        return Err("startup did not remove only validated unreferenced orphan".into());
    }
    run_marker(&fresh, &cloned_reference, "LIVE_IMAGE").await?;
    drop(cloned_reference);
    if fresh.collect().await? != vec![live_record] {
        return Err("last reconstructed reference not released".into());
    }
    Ok(())
}

#[tokio::test]
async fn fresh_repin_reference_protects_image_while_prior_orphan_is_collected() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (live_source, _) = fixture(&root, "live", "valid", "LIVE_IMAGE", 1)?;
    let (old_source, _) = fixture(&root, "orphan", "valid", "OLD_IMAGE", 2)?;
    let mut first = ImageRegistry::new(&root).await?;
    let live = first.pin(&live_source, &expected_build(1)?).await?;
    let orphan = first.pin(&old_source, &expected_build(2)?).await?;
    let orphan_record = orphan.image().clone();
    let live_record = live.image().clone();
    drop(live);
    drop(orphan);
    drop(first);
    let mut fresh = ImageRegistry::new(&root).await?;
    let live = fresh.pin(&live_source, &expected_build(1)?).await?;
    if live.image() != &live_record
        || fresh.collect().await? != vec![orphan_record.clone()]
        || orphan_record.retained_path().exists()
    {
        return Err("fresh re-pin did not rebuild collection reference".into());
    }
    std::fs::remove_file(live_source)?;
    run_marker(&fresh, &live, "LIVE_IMAGE").await
}

#[tokio::test]
async fn fresh_repin_cannot_weaken_carried_inode_authority() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "inode", "valid", "SAME_BYTES", 1)?;
    let mut first = ImageRegistry::new(&root).await?;
    let lease = first.pin(&source, &expected_build(1)?).await?;
    let carried = lease.image().clone();
    drop(lease);
    drop(first);
    std::fs::remove_file(carried.retained_path())?;
    std::fs::copy(&source, carried.retained_path())?;
    let mut fresh = ImageRegistry::new(&root).await?;
    if !matches!(
        fresh.reacquire(carried.clone(), expected_build(1)?).await,
        Err(ImageError::ImageUnavailable)
    ) {
        return Err("carried old inode was silently replaced".into());
    }
    let lease = fresh.pin(&source, &expected_build(1)?).await?;
    if lease.image().inode() == carried.inode()
        || lease.image().file_sha256() != carried.file_sha256()
    {
        return Err("fresh pin did not capture actual same-code replacement inode".into());
    }
    if fresh.reacquire(carried, expected_build(1)?).await.is_ok() {
        return Err("fresh pin granted old carried record authority".into());
    }
    run_marker(&fresh, &lease, "SAME_BYTES").await
}

#[tokio::test]
async fn fresh_collection_preserves_invalid_and_unrecognized_nodes() -> TestResult {
    for defect in [
        "unknown-file",
        "unknown-dir",
        "uppercase",
        "wrong-digest",
        "directory-symlink",
        "file-symlink",
        "extra-child",
        "nonprivate",
        "no-exec",
    ] {
        let temp = root()?;
        let root = private_root(&temp)?;
        let (source, _) = fixture(&root, "foreign", "valid", "FOREIGN", 1)?;
        let captured = capture(&source, true).await?;
        let registry = ImageRegistry::new(&root).await?;
        let canonical = registry.images_root.join(digest_hex(&captured.digest));
        let directory = match defect {
            "unknown-file" | "unknown-dir" => registry.images_root.join("unknown"),
            "uppercase" => registry
                .images_root
                .join(digest_hex(&captured.digest).to_uppercase()),
            "wrong-digest" => registry.images_root.join("00".repeat(32)),
            _ => canonical.clone(),
        };
        match defect {
            "unknown-file" => std::fs::write(&directory, b"PRESERVE")?,
            "directory-symlink" => std::os::unix::fs::symlink(&root, &directory)?,
            _ => {
                ensure_private(&directory)?;
                let retained = directory.join("codex-router");
                if defect == "file-symlink" {
                    std::os::unix::fs::symlink(&source, &retained)?;
                } else {
                    std::fs::copy(&source, &retained)?;
                }
                if defect == "extra-child" {
                    std::fs::write(directory.join("foreign"), b"PRESERVE")?;
                }
                if defect == "nonprivate" {
                    std::fs::set_permissions(&directory, Permissions::from_mode(0o755))?;
                }
                if defect == "no-exec" {
                    std::fs::set_permissions(&retained, Permissions::from_mode(0o644))?;
                }
            }
        }
        let before = std::fs::symlink_metadata(&directory)?;
        drop(registry);
        let mut fresh = ImageRegistry::new(&root).await?;
        if !fresh.collect().await?.is_empty() {
            return Err(format!("foreign startup node collected: {defect}").into());
        }
        let after = std::fs::symlink_metadata(&directory)?;
        if before.ino() != after.ino() || before.dev() != after.dev() {
            return Err("foreign disk node replaced".into());
        }
        if defect == "unknown-file" {
            if std::fs::read(&directory)? != b"PRESERVE" {
                return Err("unknown file changed".into());
            }
        } else if defect != "directory-symlink"
            && !std::fs::symlink_metadata(directory.join("codex-router"))?.is_symlink()
            && std::fs::read(directory.join("codex-router"))? != captured.bytes
        {
            return Err("foreign retained bytes changed".into());
        }
        eprintln!("STARTUP_PRESERVED defect={defect} inode={}", after.ino());
    }
    Ok(())
}

#[tokio::test]
async fn fresh_repin_preserves_nonmatching_existing_nodes_without_exposure() -> TestResult {
    for defect in [
        "content",
        "permissions",
        "extra-child",
        "symlink",
        "nonprivate",
    ] {
        let temp = root()?;
        let root = private_root(&temp)?;
        let (source, proof) = fixture(&root, "source", "valid", "SOURCE", 1)?;
        let captured = capture(&source, true).await?;
        let registry = ImageRegistry::new(&root).await?;
        let directory = registry.images_root.join(digest_hex(&captured.digest));
        ensure_private(&directory)?;
        let retained = directory.join("codex-router");
        if defect == "symlink" {
            std::os::unix::fs::symlink(&source, &retained)?;
        } else {
            std::fs::copy(&source, &retained)?;
            match defect {
                "content" => std::fs::write(&retained, b"#!/bin/sh\necho FOREIGN\n")?,
                "permissions" => {
                    std::fs::set_permissions(&retained, Permissions::from_mode(0o555))?
                }
                "extra-child" => std::fs::write(directory.join("foreign"), b"PRESERVE")?,
                "nonprivate" => {
                    std::fs::set_permissions(&directory, Permissions::from_mode(0o755))?
                }
                _ => return Err("unknown node defect".into()),
            }
        }
        let before = std::fs::symlink_metadata(&retained)?;
        let bytes = std::fs::read(&retained)?;
        drop(registry);
        let mut fresh = ImageRegistry::new(&root).await?;
        if fresh.pin(&source, &expected_build(1)?).await.is_ok() || proof.exists() {
            return Err(format!("foreign existing node exposed or executed: {defect}").into());
        }
        let after = std::fs::symlink_metadata(&retained)?;
        if before.dev() != after.dev()
            || before.ino() != after.ino()
            || before.mode() != after.mode()
            || std::fs::read(&retained)? != bytes
        {
            return Err("foreign node mutated on re-pin refusal".into());
        }
        eprintln!("REPIN_PRESERVED defect={defect} inode={}", after.ino());
    }
    Ok(())
}

#[tokio::test]
async fn discovered_image_stays_referenced_by_pending_warmup_until_real_empty_and_reap()
-> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "pending-existing", "valid", "PENDING_EXISTING", 1)?;
    let mut first = ImageRegistry::new(&root).await?;
    let lease = first.pin(&source, &expected_build(1)?).await?;
    let record = lease.image().clone();
    drop(lease);
    drop(first);
    let mut fresh = ImageRegistry::new(&root).await?;
    let guard = OwnedImageNode::observed_existing(&std::fs::metadata(record.retained_path())?);
    let mut command = Command::new(record.retained_path());
    command.arg("hold").stdout(Stdio::piped());
    let mut group =
        crate::owned_launch_test_support::require_launched(OwnedProcessGroup::spawn(command).await)
            .await?;
    let mut output = BufReader::new(group.take_stdout().ok_or("pending stdout absent")?);
    let mut ready = String::new();
    timeout(Duration::from_secs(3), output.read_line(&mut ready)).await??;
    if !ready.starts_with("PENDING_EXISTING PID ") {
        return Err("pending retained code not ready".into());
    }
    fresh.pending_warmups.push(PendingWarmup {
        cleanup: crate::image_warmup::WarmupCleanup::Running(group),
        _image: guard,
    });
    if !fresh.collect().await?.is_empty() || !record.retained_path().exists() {
        return Err("discovery removed pending warmup reference".into());
    }
    let pending = fresh
        .pending_warmups
        .first_mut()
        .ok_or("pending owner lost")?;
    pending
        .cleanup
        .running_group_mut()?
        .begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let stopped = pending
        .cleanup
        .running_group_mut()?
        .wait_for_stop(&CancellationToken::new())
        .await?;
    if !matches!(stopped, GroupStopStatus::GroupEmpty { .. })
        || pending
            .cleanup
            .running_group()?
            .leader_exit_status()
            .is_none()
    {
        return Err("pending retained cleanup fence fabricated".into());
    }
    if fresh.collect().await? != vec![record.clone()] || record.retained_path().exists() {
        return Err("pending image not released after actual empty/reap".into());
    }
    Ok(())
}

#[path = "retained_image_refusal_tests.rs"]
mod refusal_tests;
