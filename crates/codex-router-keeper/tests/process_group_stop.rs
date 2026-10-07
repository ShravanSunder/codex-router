use codex_router_keeper::{
    GroupLaunchOutcome, GroupStopStatus, GroupStopTiming, OwnedProcessGroup,
};
use tokio::time::{Duration, Instant, timeout};
#[path = "support/group_fixture.rs"]
mod group_fixture;
use group_fixture::{TestResult, read_ready, ready_command};
#[tokio::test]
async fn cooperative_group_term_is_reaped_and_esrch_before_completion() -> TestResult {
    let command = ready_command("cooperative")?;
    let mut group = require_launched(OwnedProcessGroup::spawn(command).await).await?;
    let mut output = group.take_stdout().ok_or("fixture output absent")?;
    let ready = read_ready(&mut output).await?;
    if ready.pid != group.leader_pid().as_pid().as_raw_pid()
        || ready.group != group.process_group_id().as_pid().as_raw_pid()
    {
        return Err("launch did not establish actual distinct group".into());
    }
    group.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let result = timeout(
        Duration::from_secs(4),
        group.wait_for_stop(&tokio_util::sync::CancellationToken::new()),
    )
    .await??;
    if !matches!(result, GroupStopStatus::GroupEmpty { .. })
        || group.leader_exit_status().is_none()
        || rustix::process::test_kill_process_group(group.process_group_id().as_pid())
            != Err(rustix::io::Errno::SRCH)
    {
        return Err("leader status was substituted for group empty".into());
    }
    Ok(())
}
#[tokio::test]
#[ignore = "compiled child entrypoint executed by permanent group scenarios"]
async fn group_fixture_entrypoint() -> TestResult {
    group_fixture::run_fixture().await
}

use codex_router_keeper::{GroupStopError, GroupStopProfile, GroupStopResult};
use tokio_util::sync::CancellationToken;
async fn start_fixture(
    mode: &str,
) -> Result<
    (
        OwnedProcessGroup,
        group_fixture::FixtureReady,
        tokio::process::ChildStdout,
    ),
    Box<dyn std::error::Error + Send + Sync>,
> {
    let command = ready_command(mode)?;
    let mut group = require_launched(OwnedProcessGroup::spawn(command).await).await?;
    let mut output = group.take_stdout().ok_or("fixture output absent")?;
    let ready = read_ready(&mut output).await?;
    if ready.pid != group.leader_pid().as_pid().as_raw_pid()
        || ready.group != ready.pid
        || ready.group == rustix::process::getpgrp().as_raw_pid()
    {
        return Err("fixture group was not its own leader".into());
    }
    Ok((group, ready, output))
}
#[tokio::test]
async fn ignored_term_requires_real_group_kill_and_leader_reap() -> TestResult {
    use std::os::unix::process::ExitStatusExt;
    let (mut group, _, _output) = start_fixture("ignore").await?;
    let timing = GroupStopTiming::shortened(
        GroupStopProfile::Normal,
        Duration::from_millis(80),
        Duration::from_secs(1),
    )?;
    group.begin_stop(timing, Instant::now())?;
    let result = timeout(
        Duration::from_secs(3),
        group.wait_for_stop(&CancellationToken::new()),
    )
    .await??;
    if result
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
        || group
            .leader_exit_status()
            .and_then(|status| status.signal())
            != Some(9)
    {
        return Err("ignoring fixture was not killed/reaped".into());
    }
    if rustix::process::test_kill_process_group(group.process_group_id().as_pid())
        != Err(rustix::io::Errno::SRCH)
    {
        return Err("killed group still exists".into());
    }
    Ok(())
}
#[tokio::test]
async fn cooperative_family_is_empty_only_after_parent_reaps_descendant() -> TestResult {
    let (mut group, ready, _output) = start_fixture("family").await?;
    let descendant = codex_router_keeper_protocol::ChildPid::new(i64::from(
        ready.descendant.ok_or("descendant absent")?,
    ))?;
    group.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    let result = timeout(
        Duration::from_secs(4),
        group.wait_for_stop(&CancellationToken::new()),
    )
    .await??;
    if result
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Graceful,
        })
        || rustix::process::test_kill_process(descendant.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(group.process_group_id().as_pid())
            != Err(rustix::io::Errno::SRCH)
    {
        return Err("family completed before descendant/group ESRCH".into());
    }
    Ok(())
}
#[tokio::test]
async fn already_reaped_leader_with_live_descendant_is_not_a_completed_group() -> TestResult {
    use tokio::io::AsyncWriteExt;
    let (mut group, ready, _output) = start_fixture("parent-exits").await?;
    let descendant = codex_router_keeper_protocol::ChildPid::new(i64::from(
        ready.descendant.ok_or("descendant absent")?,
    ))?;
    let mut input = group.take_stdin().ok_or("leader stdin absent")?;
    input.write_all(b"E").await?;
    drop(input);
    timeout(Duration::from_secs(2), async {
        while group.leader_exit_status().is_none() {
            if matches!(
                group.tick(Instant::now())?,
                GroupStopStatus::GroupEmpty { .. }
            ) {
                return Err("leader reap falsely completed live group".into());
            }
            tokio::task::yield_now().await;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await??;
    rustix::process::test_kill_process(descendant.as_pid())?;
    rustix::process::test_kill_process_group(group.process_group_id().as_pid())?;
    group.begin_stop(
        GroupStopTiming::shortened(
            GroupStopProfile::Normal,
            Duration::from_millis(80),
            Duration::from_secs(2),
        )?,
        Instant::now(),
    )?;
    let result = timeout(
        Duration::from_secs(4),
        group.wait_for_stop(&CancellationToken::new()),
    )
    .await??;
    if result
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
        || rustix::process::test_kill_process_group(group.process_group_id().as_pid())
            != Err(rustix::io::Errno::SRCH)
    {
        return Err(format!("orphan descendant group not ESRCH after stop: {result:?}").into());
    }
    Ok(())
}
#[tokio::test]
async fn dropped_and_cancelled_waiter_preserve_original_timer_and_owned_cleanup() -> TestResult {
    let (mut group, _, _output) = start_fixture("ignore").await?;
    let timing = GroupStopTiming::shortened(
        GroupStopProfile::Normal,
        Duration::from_millis(80),
        Duration::from_secs(1),
    )?;
    group.begin_stop(timing, Instant::now())?;
    let original = *group.progress();
    if !matches!(
        group.begin_stop(GroupStopTiming::forced_handover(), Instant::now()),
        Err(GroupStopError::StopAlreadyRequested)
    ) || *group.progress() != original
    {
        return Err("illegal profile switch reset progress".into());
    }
    let cancel = CancellationToken::new();
    let mut waiting = Box::pin(group.wait_for_stop(&cancel));
    tokio::select! {biased; result=&mut waiting=>return Err(format!("wait settled before drop: {result:?}").into()), ()=tokio::task::yield_now()=>{}}
    drop(waiting);
    if *group.progress() != original {
        return Err("dropping waiter reset progress".into());
    }
    cancel.cancel();
    if !matches!(
        group.wait_for_stop(&cancel).await,
        Err(GroupStopError::Cancelled)
    ) || *group.progress() != original
    {
        return Err("cancellation reset/lost owner progress".into());
    }
    group.begin_stop(timing, Instant::now())?;
    if *group.progress() != original {
        return Err("re-request reset original timer".into());
    }
    let result = timeout(
        Duration::from_secs(3),
        group.wait_for_stop(&CancellationToken::new()),
    )
    .await??;
    if result
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
    {
        return Err("retained owner could not finish after cancellation".into());
    }
    Ok(())
}
#[tokio::test]
async fn forced_profile_uses_real_kill_and_group_empty_remains_separate() -> TestResult {
    use std::os::unix::process::ExitStatusExt;
    let (mut group, _, _output) = start_fixture("ignore").await?;
    group.begin_stop(GroupStopTiming::forced_handover(), Instant::now())?;
    let result = timeout(
        Duration::from_secs(3),
        group.wait_for_stop(&CancellationToken::new()),
    )
    .await??;
    if result
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Killed,
        })
        || group
            .leader_exit_status()
            .and_then(|status| status.signal())
            != Some(9)
    {
        return Err("forced real kill did not empty/reap".into());
    }
    Ok(())
}
#[tokio::test]
async fn provider_style_eof_exit_is_empty_without_extra_stop_signal() -> TestResult {
    let (mut group, _, _output) = start_fixture("eof").await?;
    drop(group.take_stdin());
    let status = timeout(Duration::from_secs(2), async {
        let poll=codex_router_keeper::lifecycle_bounds::GROUP_POLL_INTERVAL;
        let mut ticks=tokio::time::interval_at(Instant::now()+poll,poll);
        loop {
            ticks.tick().await;
            let status = match group.tick(Instant::now()) {
                Ok(status)=>status,
                Err(error)=> {
                    eprintln!("EOF_PROBE_FAILURE pid={:?} pgid={:?} leader={:?} progress={:?} process_probe={:?} group_probe={:?} getpgid={:?}",group.leader_pid(),group.process_group_id(),group.leader_exit_status(),group.progress(),rustix::process::test_kill_process(group.leader_pid().as_pid()),rustix::process::test_kill_process_group(group.process_group_id().as_pid()),rustix::process::getpgid(Some(group.leader_pid().as_pid())));
                    return Err(error.into());
                }
            };
            if matches!(status, GroupStopStatus::GroupEmpty { .. })
                && group.leader_exit_status().is_some()
            {
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(status);
            }
        }
    })
    .await??;
    if status
        != (GroupStopStatus::GroupEmpty {
            result: GroupStopResult::Graceful,
        })
    {
        return Err("EOF fixture not clean empty".into());
    }
    let before = *group.progress();
    group.begin_stop(GroupStopTiming::normal(), Instant::now())?;
    if *group.progress() != before || group.tick(Instant::now())? != status {
        return Err("already empty group was signalled/restarted".into());
    }
    Ok(())
}

async fn require_launched(
    outcome: GroupLaunchOutcome,
) -> Result<OwnedProcessGroup, Box<dyn std::error::Error + Send + Sync>> {
    match outcome {
        GroupLaunchOutcome::Launched(group) => Ok(group),
        GroupLaunchOutcome::Refused { reason } => Err(reason.into()),
        GroupLaunchOutcome::CleanupPending {
            reason,
            mut cleanup,
        } => {
            cleanup.wait_for_cleanup(&CancellationToken::new()).await?;
            Err(reason.into())
        }
    }
}
