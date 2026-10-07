use super::*;
fn source_swap_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source.with_extension("replacement"), source)?;
    std::fs::hard_link(source, target)
}
fn source_change_link(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::hard_link(source, target)?;
    std::fs::write(source, b"#!/bin/sh\necho CHANGED\n")
}
#[tokio::test]
async fn source_path_swap_and_changed_bytes_refuse_and_clean_only_owned_links() -> TestResult {
    for link in [
        source_swap_link as LinkOperation,
        source_change_link as LinkOperation,
    ] {
        let temp = root()?;
        let root = private_root(&temp)?;
        let (source, _) = fixture(&root, "race", "valid", "IMAGE_A", 1)?;
        std::fs::copy(&source, source.with_extension("replacement"))?;
        let mut registry = ImageRegistry::new(&root).await?;
        if !matches!(
            registry
                .pin_with_link(&source, &expected_build(1)?, link, PREPARE_DEADLINE)
                .await,
            Err(ImageError::ImageUnavailable)
        ) {
            return Err("source race was not rejected".into());
        }
        for directory in std::fs::read_dir(&registry.images_root)? {
            if std::fs::read_dir(directory?.path())?.next().is_some() {
                return Err("raced retained link was exposed after refusal".into());
            }
        }
        if !source.exists() {
            return Err("source removed by retained cleanup".into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn prepare_budget_is_literal_thirty_seconds_and_tests_cannot_expand_it() -> TestResult {
    if PREPARE_DEADLINE != Duration::from_secs(30) {
        return Err("production prepare deadline changed".into());
    }
    for budget in [Duration::ZERO, Duration::from_secs(31)] {
        let temp = root()?;
        let root = private_root(&temp)?;
        let (source, proof) = fixture(&root, "budget", "valid", "IMAGE_A", 1)?;
        let mut registry = ImageRegistry::new(&root).await?;
        if !matches!(
            registry
                .pin_with_link(&source, &expected_build(1)?, exdev, budget)
                .await,
            Err(ImageError::InvalidBudget)
        ) || proof.exists()
        {
            return Err("invalid budget launched or accepted".into());
        }
    }
    Ok(())
}
#[tokio::test]
async fn collection_does_not_remove_unknown_child_inside_owned_digest_directory() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "source", "valid", "IMAGE_A", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry.pin(&source, &expected_build(1)?).await?;
    let path = lease.image().retained_path().to_owned();
    let foreign = path.parent().ok_or("digest parent")?.join("unknown");
    std::fs::write(&foreign, b"keep")?;
    drop(lease);
    if !matches!(registry.collect().await, Err(ImageError::ForeignNode))
        || !path.exists()
        || std::fs::read(foreign)? != b"keep"
    {
        return Err("collection removed foreign node or its sibling".into());
    }
    Ok(())
}
#[tokio::test]
async fn readonly_source_is_never_chmodded_by_retention() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, _) = fixture(&root, "readonly", "valid", "IMAGE_A", 1)?;
    std::fs::set_permissions(&source, Permissions::from_mode(0o555))?;
    let mut registry = ImageRegistry::new(&root).await?;
    let lease = registry.pin(&source, &expected_build(1)?).await?;
    if std::fs::metadata(&source)?.mode() & 0o777 != 0o555
        || std::fs::metadata(lease.image().retained_path())?.mode() & 0o777 != 0o555
    {
        return Err("retention changed source inode permissions".into());
    }
    run_marker(&registry, &lease, "IMAGE_A").await
}

#[tokio::test]
async fn unfinished_warmup_reference_keeps_inode_until_real_group_empty_and_reap() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (candidate, _) = fixture(&root, "pending", "valid", "PENDING", 1)?;
    let mut registry = ImageRegistry::new(&root).await?;
    let guard = OwnedImageNode::new(candidate.clone(), &std::fs::metadata(&candidate)?);
    let mut command = Command::new(&candidate);
    command.arg("hold").stdout(Stdio::piped());
    let mut group = OwnedProcessGroup::spawn(command).await?;
    let mut output = BufReader::new(group.take_stdout().ok_or("pending stdout absent")?);
    let mut ready = String::new();
    timeout(Duration::from_secs(3), output.read_line(&mut ready)).await??;
    if !ready.starts_with("PENDING PID ") {
        return Err("pending warmup code not ready".into());
    }
    registry.pending_warmups.push(PendingWarmup {
        group,
        _image: guard,
    });
    registry.collect().await?;
    if !candidate.exists() || registry.pending_warmups.len() != 1 {
        return Err("live warmup inode/owner lost".into());
    }
    let pending = registry
        .pending_warmups
        .first_mut()
        .ok_or("pending owner lost")?;
    pending
        .group
        .begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let stopped = pending
        .group
        .wait_for_stop(&CancellationToken::new())
        .await?;
    if !matches!(stopped, GroupStopStatus::GroupEmpty { .. })
        || pending.group.leader_exit_status().is_none()
    {
        return Err("pending empty/reap fence fabricated".into());
    }
    registry.collect().await?;
    if candidate.exists() || !registry.pending_warmups.is_empty() {
        return Err("empty warmup inode not released".into());
    }
    Ok(())
}

#[tokio::test]
async fn startup_gate_wait_counts_against_prepare_and_late_launch_is_reaped() -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "startup", "valid", "IMAGE_A", 1)?;
    let captured = capture(&source, true).await?;
    let mut registry = ImageRegistry::new(&root).await?;
    // Pin preparation itself uses shared creation guards, so isolate the actual warmup
    // launch boundary for the deadline oracle while holding its exclusive gate.
    let gate = DescriptorGate::global().spawn().await;
    let expected = expected_build(1)?;
    let preparing = warmup(&captured.path, &expected, Duration::from_millis(20));
    let release = async {
        tokio::time::sleep_until(Instant::now() + Duration::from_millis(80)).await;
        drop(gate);
    };
    let (outcome, ()) = tokio::join!(preparing, release);
    let WarmupOutcome::Refused {
        reason: ImageError::WarmupTimedOut,
        finished_pid: Some(pid),
    } = outcome
    else {
        return Err("startup outside deadline accepted or ownership lost".into());
    };
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG),
            Err(rustix::io::Errno::CHILD)
        )
    {
        return Err("late warmup did not actually reap/empty its exact child".into());
    }
    eprintln!("LATE_WARMUP_REAP pid={pid:?} process=ESRCH group=ESRCH wait=ECHILD");
    if proof.exists()
        && std::fs::read_to_string(proof)?.parse::<i32>()? != pid.as_pid().as_raw_pid()
    {
        return Err("late script PID differs from actual owner".into());
    }
    if !registry.collect().await?.is_empty() {
        return Err("late failed warmup registered an image".into());
    }
    Ok(())
}
