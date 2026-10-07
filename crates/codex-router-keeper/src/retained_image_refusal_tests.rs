use super::*;
use codex_router_keeper_protocol::ChildPid;
use std::os::unix::process::ExitStatusExt;
use tokio::time::timeout_at;

struct RefusalEvidence<'a> {
    mode: &'a str,
    record: &'a SlotImage,
    bytes: &'a [u8],
    pid: ChildPid,
}
fn preserved_image(evidence: &RefusalEvidence<'_>) -> TestResult {
    let metadata = std::fs::symlink_metadata(evidence.record.retained_path())?;
    if !metadata.is_file()
        || metadata.dev() != evidence.record.device()
        || metadata.ino() != evidence.record.inode()
        || std::fs::read(evidence.record.retained_path())? != evidence.bytes
    {
        return Err("refused existing image identity/bytes changed before cleanup fence".into());
    }
    Ok(())
}
fn completed_cause(mode: &str, result: &Result<ImageLease, ImageError>) -> bool {
    match mode {
        "mismatch" | "version" => matches!(result, Err(ImageError::BuildInfoMismatch)),
        "invalid" | "trailing" => matches!(result, Err(ImageError::BuildInfoJson(_))),
        "nonzero" => matches!(result, Err(ImageError::WarmupExit)),
        "oversized" => matches!(result, Err(ImageError::WarmupTooLarge)),
        "timeout" => matches!(result, Err(ImageError::WarmupTimedOut)),
        _ => false,
    }
}
fn pending_identity(registry: &ImageRegistry, evidence: &RefusalEvidence<'_>) -> TestResult {
    let [pending] = registry.pending_warmups.as_slice() else {
        return Err("cleanup failure did not retain exactly one owned child/image".into());
    };
    if pending.group.leader_pid() != evidence.pid
        || pending.group.process_group_id().as_pid() != evidence.pid.as_pid()
        || pending._image.device != evidence.record.device()
        || pending._image.inode != evidence.record.inode()
        || !registry.entries.is_empty()
    {
        return Err(
            "pending failure debt differs from exact child/image or exposed ready entry".into(),
        );
    }
    preserved_image(evidence)
}
fn actual_reap_fence(pid: ChildPid) -> TestResult {
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(Some(pid.as_pid()), rustix::process::WaitOptions::NOHANG),
            Err(rustix::io::Errno::CHILD)
        )
    {
        return Err(
            "refused warmup missing actual process/group ESRCH or exact-child ECHILD".into(),
        );
    }
    Ok(())
}
async fn prove_refusal(
    registry: &mut ImageRegistry,
    evidence: RefusalEvidence<'_>,
    result: Result<ImageLease, ImageError>,
) -> TestResult {
    preserved_image(&evidence)?;
    if !registry.entries.is_empty() || result.is_ok() {
        return Err("refused producer exposed image lease/ready entry".into());
    }
    if registry.pending_warmups.is_empty() {
        if !completed_cause(evidence.mode, &result) {
            return Err(format!("wrong completed refusal: {} {result:?}", evidence.mode).into());
        }
        actual_reap_fence(evidence.pid)?;
        eprintln!(
            "COMPLETED_REFUSAL mode={} pid={:?} result={result:?} image_preserved process=ESRCH group=ESRCH wait=ECHILD",
            evidence.mode, evidence.pid
        );
        return Ok(());
    }
    if !matches!(
        result,
        Err(ImageError::Process(crate::GroupStopError::Probe(
            rustix::io::Errno::PERM
        )))
    ) {
        return Err(format!("unrecognized cleanup-pending failure: {result:?}").into());
    }
    pending_identity(registry, &evidence)?;
    let pending = registry
        .pending_warmups
        .first()
        .ok_or("pending owner missing")?;
    let deadline = match *pending.group.progress() {
        crate::GroupStopProgress::TermSent { at, timing } => {
            at + timing.term_grace + timing.kill_observe
        }
        crate::GroupStopProgress::KillSent { at, timing } => at + timing.kill_observe,
        _ => return Err("cleanup debt has no original bounded stop in progress".into()),
    };
    eprintln!(
        "PENDING_REFUSAL mode={} pid={:?} error={result:?} stored={:?} progress={:?}",
        evidence.mode,
        evidence.pid,
        pending.group.leader_exit_status(),
        pending.group.progress()
    );
    timeout_at(deadline, async {
        let cadence = crate::lifecycle_bounds::GROUP_POLL_INTERVAL;
        let mut ticks = tokio::time::interval_at(Instant::now()+cadence,cadence);
        loop {
            ticks.tick().await;
            pending_identity(registry, &evidence)?;
            let collected = registry.collect().await;
            match collected {
                Ok(removed) if registry.pending_warmups.is_empty() => {
                    if removed!=vec![evidence.record.clone()] || evidence.record.retained_path().exists() {
                        return Err("pending collection did not remove only fenced unreferenced image".into());
                    }
                    let [(pid,status)] = registry.warmup_reap_observations.as_slice() else {
                        return Err("sole-owner stored reap status lost before retirement".into());
                    };
                    if *pid!=evidence.pid || !(status.signal()==Some(15) || status.signal()==Some(9) || (evidence.mode=="nonzero" && status.code()==Some(3))) {
                        return Err("stored terminal status differs from exact refused fixture".into());
                    }
                    actual_reap_fence(evidence.pid)?;
                    eprintln!("PENDING_FENCE mode={} pid={pid:?} stored={status:?} pending=0 process=ESRCH group=ESRCH wait=ECHILD", evidence.mode);
                    return Ok(());
                }
                Ok(removed) => {
                    if !removed.is_empty() { return Err("pending inode removed before real fence".into()); }
                    pending_identity(registry, &evidence)?;
                }
                Err(ImageError::Process(crate::GroupStopError::Probe(rustix::io::Errno::PERM))) => {
                    pending_identity(registry, &evidence)?;
                    eprintln!("PENDING_COLLECT_ERROR pid={:?} precise=ProbePERM owner_and_image_retained", evidence.pid);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }).await?
}
async fn refusal_case(mode: &str) -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "bad", mode, "BAD", 1)?;
    let mut first = ImageRegistry::new(&root).await?;
    let lease = first.pin(&source, &expected_build(1)?).await?;
    let record = lease.image().clone();
    let bytes = std::fs::read(record.retained_path())?;
    drop(lease);
    drop(first);
    let mut fresh = ImageRegistry::new(&root).await?;
    let result = fresh
        .pin_with_link(
            &source,
            &expected_build(1)?,
            hard_link,
            Duration::from_secs(2),
        )
        .await;
    let pid = ChildPid::new(std::fs::read_to_string(proof)?.parse()?)?;
    prove_refusal(
        &mut fresh,
        RefusalEvidence {
            mode,
            record: &record,
            bytes: &bytes,
            pid,
        },
        result,
    )
    .await
}
#[tokio::test]
async fn fresh_warmup_refusal_preserves_preexisting_node_and_reaps_child() -> TestResult {
    for mode in [
        "mismatch",
        "version",
        "invalid",
        "trailing",
        "nonzero",
        "oversized",
        "timeout",
    ] {
        refusal_case(mode).await?;
    }
    Ok(())
}
#[tokio::test]
async fn trailing_refusal_reaches_original_cleanup_fence() -> TestResult {
    refusal_case("trailing").await
}
