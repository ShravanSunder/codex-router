use crate::lifecycle_bounds::PREPARE_DEADLINE;
use crate::{
    GroupStopStatus, GroupStopTiming, ImageError, OwnedProcessGroup,
    lifecycle_bounds::GROUP_POLL_INTERVAL,
};
use codex_router_keeper_protocol::{BuildInfo, MAX_FRAME_BYTES};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::Command,
    time::{Instant, timeout_at},
};
use tokio_util::sync::CancellationToken;
pub(crate) enum WarmupOutcome {
    Verified,
    Refused {
        reason: ImageError,
        finished_pid: Option<codex_router_keeper_protocol::ChildPid>,
    },
    CleanupPending {
        group: OwnedProcessGroup,
        failure: ImageError,
    },
}
pub(crate) async fn warmup(path: &Path, expected: &BuildInfo, budget: Duration) -> WarmupOutcome {
    if budget.is_zero() || budget > PREPARE_DEADLINE {
        return WarmupOutcome::Refused {
            reason: ImageError::InvalidBudget,
            finished_pid: None,
        };
    }
    let deadline = Instant::now() + budget;
    let mut command = Command::new(path);
    command
        .args(["build-info", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut launching = Box::pin(OwnedProcessGroup::spawn(command));
    // A missed deadline refuses readiness but never abandons queued launch ownership.
    let mut group = match timeout_at(deadline, &mut launching).await {
        Ok(Ok(group)) => group,
        Ok(Err(error)) => {
            return WarmupOutcome::Refused {
                reason: error.into(),
                finished_pid: None,
            };
        }
        Err(_) => {
            return match launching.await {
                Ok(group) => reject_and_cleanup(group, ImageError::WarmupTimedOut).await,
                Err(error) => WarmupOutcome::Refused {
                    reason: error.into(),
                    finished_pid: None,
                },
            };
        }
    };
    match read_and_wait(&mut group, expected, deadline).await {
        Ok(())
            if group
                .leader_exit_status()
                .is_some_and(|status| status.success()) =>
        {
            #[cfg(test)]
            eprintln!(
                "WARMUP_VERIFIED pid={:?} stored={:?} progress={:?}",
                group.leader_pid(),
                group.leader_exit_status(),
                group.progress()
            );
            WarmupOutcome::Verified
        }
        Ok(()) => reject_and_cleanup(group, ImageError::WarmupExit).await,
        Err(reason) => reject_and_cleanup(group, reason).await,
    }
}

async fn reject_and_cleanup(mut group: OwnedProcessGroup, reason: ImageError) -> WarmupOutcome {
    match cleanup(&mut group).await {
        Ok(()) => WarmupOutcome::Refused {
            reason,
            finished_pid: Some(group.leader_pid()),
        },
        Err(failure) => WarmupOutcome::CleanupPending { group, failure },
    }
}
async fn read_and_wait(
    group: &mut OwnedProcessGroup,
    expected: &BuildInfo,
    deadline: Instant,
) -> Result<(), ImageError> {
    let output = group.take_stdout().ok_or(ImageError::InvalidExecutable)?;
    let mut reading = Box::pin(async move {
        let mut limited = output.take((MAX_FRAME_BYTES + 1) as u64);
        let mut bytes = Vec::new();
        limited.read_to_end(&mut bytes).await?;
        Ok::<_, ImageError>(bytes)
    });
    let mut received_info = None;
    let mut unresolved_error = None;
    let mut ticks =
        tokio::time::interval_at(Instant::now() + GROUP_POLL_INTERVAL, GROUP_POLL_INTERVAL);
    loop {
        tokio::select! {
            result=&mut reading, if received_info.is_none()=>{
                let bytes=result?;
                if bytes.len()>MAX_FRAME_BYTES { return Err(ImageError::WarmupTooLarge); }
                let actual:BuildInfo=serde_json::from_slice(&bytes)?;
                if &actual!=expected { return Err(ImageError::BuildInfoMismatch); }
                received_info=Some(actual);
            },
            _=ticks.tick()=>{
                match group.tick(Instant::now()) {
                    Ok(_) => {
                        #[cfg(test)]
                        if unresolved_error.is_some() {
                            eprintln!("WARMUP_EXIT_OBSERVATION_CLEARED pid={:?} stored={:?} progress={:?}", group.leader_pid(), group.leader_exit_status(), group.progress());
                        }
                        unresolved_error=None;
                    }
                    Err(error) => {
                        if matches!(error, crate::GroupStopError::Probe(rustix::io::Errno::PERM))
                            && group.leader_exit_status().is_none()
                            && hold_owned_exit_observation(&error, group.leader_exit_status(), rustix::process::getpgid(Some(group.leader_pid().as_pid())))
                        {
                            #[cfg(test)]
                            eprintln!("WARMUP_EXIT_OBSERVATION_HELD pid={:?} error={error:?} stored={:?} exact_getpgid=SRCH deadline={deadline:?}", group.leader_pid(), group.leader_exit_status());
                            unresolved_error=Some(error);
                        } else {
                            return Err(error.into());
                        }
                    }
                }
            },
            _=tokio::time::sleep_until(deadline)=>return Err(failure_at_deadline(unresolved_error)),
        }
        if completion_fence_observed(
            received_info.as_ref(),
            unresolved_error.as_ref(),
            group.leader_exit_status(),
            group.progress(),
        ) {
            return Ok(());
        }
    }
}

async fn cleanup(group: &mut OwnedProcessGroup) -> Result<(), ImageError> {
    group.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let stopped = group.wait_for_stop(&CancellationToken::new()).await?;
    if !matches!(stopped, GroupStopStatus::GroupEmpty { .. })
        || group.leader_exit_status().is_none()
    {
        return Err(ImageError::CleanupIncomplete);
    }
    Ok(())
}

// An unfindable owned leader need not be waitable yet. This holds uncertainty
// inside this completion wait; it does not change the group's error or empty fence.
fn hold_owned_exit_observation(
    error: &crate::GroupStopError,
    leader_exit: Option<std::process::ExitStatus>,
    owned_lookup: rustix::io::Result<rustix::process::Pid>,
) -> bool {
    matches!(error, crate::GroupStopError::Probe(rustix::io::Errno::PERM))
        && leader_exit.is_none()
        && matches!(owned_lookup, Err(rustix::io::Errno::SRCH))
}
fn failure_at_deadline(unresolved_error: Option<crate::GroupStopError>) -> ImageError {
    match unresolved_error {
        Some(error) => error.into(),
        None => ImageError::WarmupTimedOut,
    }
}
fn completion_fence_observed(
    info: Option<&BuildInfo>,
    unresolved_error: Option<&crate::GroupStopError>,
    leader_exit: Option<std::process::ExitStatus>,
    progress: &crate::GroupStopProgress,
) -> bool {
    info.is_some()
        && unresolved_error.is_none()
        && leader_exit.is_some()
        && matches!(progress, crate::GroupStopProgress::GroupEmpty { .. })
}
#[cfg(test)]
#[path = "image_warmup_observation_tests.rs"]
mod observation_tests;
