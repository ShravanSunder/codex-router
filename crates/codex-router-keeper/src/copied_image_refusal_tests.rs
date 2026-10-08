use super::*;
use crate::image_warmup::{WarmupObservedStage, WarmupRejectionKind, WarmupTestObservation};
use std::{os::unix::process::ExitStatusExt, process::ExitStatus};
use tokio::time::timeout_at;

fn expected_rejection(mode: &str) -> Result<WarmupRejectionKind, &'static str> {
    match mode {
        "mismatch" | "version" => Ok(WarmupRejectionKind::BuildInfoMismatch),
        "tamper" => Ok(WarmupRejectionKind::ImageUnavailable),
        "invalid" | "trailing" => Ok(WarmupRejectionKind::BuildInfoJson),
        "nonzero" => Ok(WarmupRejectionKind::WarmupExit),
        "oversized" => Ok(WarmupRejectionKind::WarmupTooLarge),
        "timeout" => Ok(WarmupRejectionKind::WarmupTimedOut),
        _ => Err("unknown copied refusal mode"),
    }
}
fn completed_cause(mode: &str, result: &Result<ImageLease, ImageError>) -> bool {
    match mode {
        "mismatch" | "version" => matches!(result, Err(ImageError::BuildInfoMismatch)),
        "tamper" => matches!(result, Err(ImageError::ImageUnavailable)),
        "invalid" | "trailing" => matches!(result, Err(ImageError::BuildInfoJson(_))),
        "nonzero" => matches!(result, Err(ImageError::WarmupExit)),
        "oversized" => matches!(result, Err(ImageError::WarmupTooLarge)),
        "timeout" => matches!(result, Err(ImageError::WarmupTimedOut)),
        _ => false,
    }
}
fn terminal_status(mode: &str, status: ExitStatus) -> TestResult {
    let correct = match mode {
        "tamper" => status.success(),
        "nonzero" => status.code() == Some(3),
        "oversized" => matches!(status.signal(), Some(15 | 9)) || status.success(),
        "mismatch" | "version" | "invalid" | "trailing" | "timeout" => {
            matches!(status.signal(), Some(15 | 9))
        }
        _ => false,
    };
    if !correct {
        return Err(format!("wrong stored fixture exit for {mode}: {status:?}").into());
    }
    Ok(())
}
fn actual_fence(witness: &WarmupTestObservation) -> TestResult {
    if rustix::process::test_kill_process(witness.pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(witness.pgid.as_pid())
            != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(
                Some(witness.pid.as_pid()),
                rustix::process::WaitOptions::NOHANG
            ),
            Err(rustix::io::Errno::CHILD)
        )
    {
        return Err("copied refusal lacks real process/group ESRCH and exact-child ECHILD".into());
    }
    Ok(())
}
fn retained_candidate(
    registry: &ImageRegistry,
    witness: &WarmupTestObservation,
    bytes: &[u8],
) -> TestResult {
    let [pending] = registry.pending_warmups.as_slice() else {
        return Err("pending copied refusal lacks exactly one owner".into());
    };
    let group = pending.cleanup.running_group()?;
    let metadata = std::fs::symlink_metadata(&witness.image_path)?;
    if group.leader_pid() != witness.pid
        || group.process_group_id() != witness.pgid
        || !metadata.is_file()
        || metadata.dev() != pending._image.device
        || metadata.ino() != pending._image.inode
        || std::fs::read(&witness.image_path)? != bytes
        || !registry.entries.is_empty()
        || witness.image_path.file_name() != Some(std::ffi::OsStr::new(".candidate"))
        || witness.image_path.with_file_name("codex-router").exists()
    {
        return Err("copied pending child/inode/bytes changed or became ready".into());
    }
    eprintln!(
        "COPIED_OWNER pid={:?} pgid={:?} dev={} ino={} bytes={} stored={:?} progress={:?}",
        witness.pid,
        witness.pgid,
        metadata.dev(),
        metadata.ino(),
        bytes.len(),
        group.leader_exit_status(),
        group.progress()
    );
    Ok(())
}
async fn collect_original_owner(
    registry: &mut ImageRegistry,
    witness: &WarmupTestObservation,
    bytes: &[u8],
) -> Result<ExitStatus, Box<dyn std::error::Error + Send + Sync>> {
    retained_candidate(registry, witness, bytes)?;
    let pending = registry
        .pending_warmups
        .first()
        .ok_or("pending owner absent")?;
    let deadline = match *pending.cleanup.running_group()?.progress() {
        crate::GroupStopProgress::TermSent { at, timing } => {
            at + timing.term_grace + timing.kill_observe
        }
        crate::GroupStopProgress::KillSent { at, timing } => at + timing.kill_observe,
        _ => return Err("pending copy has no original stored stop deadline".into()),
    };
    eprintln!(
        "COPIED_PENDING pid={:?} original_deadline={deadline:?} remaining={:?}",
        witness.pid,
        deadline.saturating_duration_since(Instant::now())
    );
    timeout_at(deadline, async {
        let cadence = crate::lifecycle_bounds::GROUP_POLL_INTERVAL;
        let mut ticks = tokio::time::interval_at(Instant::now() + cadence, cadence);
        loop {
            ticks.tick().await;
            retained_candidate(registry, witness, bytes)?;
            match registry.collect().await {
                Ok(removed) if registry.pending_warmups.is_empty() => {
                    if !removed.is_empty()
                        || witness.image_path.exists()
                        || !registry.entries.is_empty()
                    {
                        return Err(
                            "unpublished copy returned ready/orphan image or was not unlinked"
                                .into(),
                        );
                    }
                    let [(pid, status)] = registry.warmup_reap_observations.as_slice() else {
                        return Err("copied cleanup lost sole-owner stored exact exit".into());
                    };
                    if *pid != witness.pid {
                        return Err("stored exit belongs to another child".into());
                    }
                    return Ok(*status);
                }
                Ok(removed) => {
                    if !removed.is_empty() {
                        return Err("pending copy collected before fence".into());
                    }
                }
                Err(ImageError::Process(crate::GroupStopError::Probe(rustix::io::Errno::PERM))) => {
                    eprintln!(
                        "COPIED_COLLECT_ERROR pid={:?} precise=ProbePERM still_unresolved",
                        witness.pid
                    );
                }
                Err(error) => return Err(error.into()),
            }
            retained_candidate(registry, witness, bytes)?;
        }
    })
    .await?
}
pub(super) async fn prove_copied_refusal(mode: &str) -> TestResult {
    let temp = root()?;
    let root = private_root(&temp)?;
    let (source, proof) = fixture(&root, "bad", mode, "BAD", 1)?;
    let source_bytes = std::fs::read(&source)?;
    let source_metadata = std::fs::metadata(&source)?;
    let foreign = root.join("foreign-preserve");
    std::fs::write(&foreign, b"foreign-node")?;
    let mut registry = ImageRegistry::new(&root).await?;
    let result = registry
        .pin_with_link(
            &source,
            &expected_build(1)?,
            exdev,
            if mode == "timeout" {
                Duration::from_secs(2)
            } else {
                PREPARE_DEADLINE
            },
        )
        .await;
    let witness = registry
        .last_warmup_observation
        .clone()
        .ok_or("warmup identity/stage witness absent")?;
    // Retain the original semantic error even when collection is needed first.
    let semantic_correct =
        witness.stage == WarmupObservedStage::Rejected(expected_rejection(mode)?);
    let pending = !registry.pending_warmups.is_empty();
    eprintln!(
        "COPIED_REFUSAL mode={mode} pid={:?} pgid={:?} stage={:?} stored={:?} pending={pending} result={result:?}",
        witness.pid, witness.pgid, witness.stage, witness.stored_exit
    );
    let status = if pending {
        let precise_error = matches!(
            result,
            Err(ImageError::Process(crate::GroupStopError::Probe(
                rustix::io::Errno::PERM
            )))
        );
        let status = collect_original_owner(&mut registry, &witness, &source_bytes).await?;
        if !precise_error {
            return Err(format!("unsupported pending copy error {result:?}").into());
        }
        status
    } else {
        if !completed_cause(mode, &result) {
            return Err(format!("wrong completed copied refusal {mode}: {result:?}").into());
        }
        witness
            .stored_exit
            .ok_or("immediate copied refusal lost stored exact exit")?
    };
    let proof_pid =
        codex_router_keeper_protocol::ChildPid::new(std::fs::read_to_string(&proof)?.parse()?)?;
    if !semantic_correct || proof_pid != witness.pid || witness.pgid.as_pid() != proof_pid.as_pid()
    {
        return Err(format!("copied {mode} missing original typed semantic refusal/exact fixture identity: {witness:?}").into());
    }
    terminal_status(mode, status)?;
    actual_fence(&witness)?;
    if !registry.entries.is_empty()
        || !registry.pending_warmups.is_empty()
        || witness.image_path.exists()
        || std::fs::read(&source)? != source_bytes
        || std::fs::metadata(&source)?.dev() != source_metadata.dev()
        || std::fs::metadata(&source)?.ino() != source_metadata.ino()
        || std::fs::read(&foreign)? != b"foreign-node"
    {
        return Err(
            "copy refusal exposed readiness, lost source/foreign node or retained candidate".into(),
        );
    }
    for directory in std::fs::read_dir(&registry.images_root)? {
        if std::fs::read_dir(directory?.path())?.next().is_some() {
            return Err("failed copy candidate exposed".into());
        }
    }
    eprintln!(
        "COPIED_FENCE mode={mode} pid={:?} pgid={:?} stored={status:?} pending=0 entries=0 candidate_absent source_foreign_preserved process=ESRCH group=ESRCH wait=ECHILD",
        witness.pid, witness.pgid
    );
    Ok(())
}
