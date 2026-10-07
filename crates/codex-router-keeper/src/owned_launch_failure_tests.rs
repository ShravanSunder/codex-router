use super::*;
use crate::OwnedSpawnCleanup;
use serde::{Deserialize, Serialize};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::time::{Duration, timeout};
type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct JoinedGroupWitness {
    pub(crate) marker: String,
    pub(crate) pid: ChildPid,
    pub(crate) original_group: ChildPgid,
    pub(crate) joined_group: i32,
}
pub(crate) fn fixture_command(marker: &Path) -> Result<Command, std::io::Error> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--ignored",
            "--exact",
            "owned_process_group::launch_failure_tests::joined_group_fixture",
            "--nocapture",
        ])
        .env("LAUNCH_JOIN_MARKER", marker)
        .env(
            "LAUNCH_PARENT_GROUP",
            rustix::process::getpgrp().as_raw_pid().to_string(),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Ok(command)
}
#[tokio::test]
#[ignore = "compiled process fixture executed by permanent launch failure tests"]
async fn joined_group_fixture() -> TestResult {
    let path = PathBuf::from(std::env::var_os("LAUNCH_JOIN_MARKER").ok_or("marker path absent")?);
    let pid = ChildPid::new(i64::from(rustix::process::getpid().as_raw_pid()))?;
    let original_group = ChildPgid::of_leader(pid);
    if rustix::process::getpgrp() != pid.as_pid() {
        return Err("fixture was not spawned into its original distinct group".into());
    }
    let parent = ChildPid::new(std::env::var("LAUNCH_PARENT_GROUP")?.parse::<i64>()?)?;
    rustix::process::setpgid(None, Some(parent.as_pid()))?;
    let witness = JoinedGroupWitness {
        marker: "REAL_JOINED_FIXTURE".into(),
        pid,
        original_group,
        joined_group: rustix::process::getpgrp().as_raw_pid(),
    };
    if witness.joined_group != parent.as_pid().as_raw_pid() {
        return Err("actual setpgid did not join parent group".into());
    }
    let temporary = path.with_extension("writing");
    std::fs::write(&temporary, serde_json::to_vec(&witness)?)?;
    std::fs::rename(temporary, path)?;
    std::future::pending::<()>().await;
    Ok(())
}
pub(crate) async fn cleanup_fences(
    cleanup: &mut OwnedSpawnCleanup,
    marker: &Path,
    parent_group: rustix::process::Pid,
) -> TestResult {
    let witness: JoinedGroupWitness = serde_json::from_slice(&std::fs::read(marker)?)?;
    let pid = cleanup.leader_pid().ok_or("cleanup PID absent")?;
    if witness.marker != "REAL_JOINED_FIXTURE"
        || witness.pid != pid
        || cleanup.original_process_group_id() != Some(witness.original_group)
        || witness.joined_group != parent_group.as_raw_pid()
    {
        return Err("actual child/group witness does not match retained owner".into());
    }
    let deadline = cleanup
        .cleanup_deadline()
        .ok_or("original deadline absent")?;
    let cancel = CancellationToken::new();
    let mut waiting = Box::pin(cleanup.wait_for_cleanup(&cancel));
    // Poll the actual cleanup wait into its first bounded observation interval,
    // then abandon only the future; child/image/timestamps remain in the caller.
    tokio::select! { biased;
        result=&mut waiting => return Err(format!("cleanup unexpectedly settled before waiter drop: {result:?}").into()),
        ()=tokio::task::yield_now() => {},
    }
    drop(waiting);
    if cleanup.cleanup_deadline() != Some(deadline) || cleanup.leader_exit_status().is_some() {
        return Err("dropped observation wait lost/reset recorded state".into());
    }
    cancel.cancel();
    if !matches!(
        cleanup.wait_for_cleanup(&cancel).await,
        Err(GroupStopError::Cancelled)
    ) || cleanup.cleanup_deadline() != Some(deadline)
    {
        return Err("cancelled cleanup did not preserve original owner/deadline".into());
    }
    timeout(
        Duration::from_secs(3),
        cleanup.wait_for_cleanup(&CancellationToken::new()),
    )
    .await??;
    if cleanup
        .leader_exit_status()
        .and_then(|status| status.signal())
        != Some(9)
        || rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(witness.original_group.as_pid())
            != Err(rustix::io::Errno::SRCH)
        || !matches!(
            rustix::process::waitpid(Some(pid.as_pid()), WaitOptions::NOHANG),
            Err(rustix::io::Errno::CHILD)
        )
        || rustix::process::getpgrp() != parent_group
    {
        return Err("cleanup/parent-survival fence missing".into());
    }
    rustix::process::test_kill_process_group(parent_group)?;
    rustix::process::test_kill_process(rustix::process::getpid())?;
    eprintln!(
        "OWNED_LAUNCH_FENCES witness={witness:?} status={:?} original_group=ESRCH process=ESRCH wait=ECHILD parent_group={parent_group:?}/alive deadline={deadline:?}",
        cleanup.leader_exit_status()
    );
    Ok(())
}
#[tokio::test]
async fn real_group_mismatch_retains_owner_and_never_signals_joined_parent_group() -> TestResult {
    let root = tempfile::tempdir()?;
    let marker = root.path().join("joined.json");
    let parent_group = rustix::process::getpgrp();
    let outcome = OwnedProcessGroup::spawn_with_checkpoint(
        fixture_command(&marker)?,
        PostSpawnCheckpoint::GroupChangeMarker(marker.clone()),
    )
    .await;
    let GroupLaunchOutcome::CleanupPending {
        reason: GroupStopError::GroupMismatch,
        mut cleanup,
    } = outcome
    else {
        return Err("actual setup mismatch did not return retained CleanupPending".into());
    };
    cleanup_fences(&mut cleanup, &marker, parent_group).await
}

#[tokio::test]
async fn stolen_setup_failure_status_disqualifies_without_inventing_exit() -> TestResult {
    let root = tempfile::tempdir()?;
    let marker = root.path().join("stolen.json");
    let parent_group = rustix::process::getpgrp();
    let outcome = OwnedProcessGroup::spawn_with_checkpoint(
        fixture_command(&marker)?,
        PostSpawnCheckpoint::GroupChangeMarker(marker.clone()),
    )
    .await;
    let GroupLaunchOutcome::CleanupPending {
        reason: GroupStopError::GroupMismatch,
        mut cleanup,
    } = outcome
    else {
        return Err("actual setup mismatch lost cleanup owner".into());
    };
    let pid = cleanup.leader_pid().ok_or("cleanup PID absent")?;
    // Deliberately steal this test-owned raw child's exit to prove disqualification.
    // No Tokio Child exists and no production reaper races this controlled test.
    timeout(Duration::from_secs(3), async {
        loop {
            if let Some((actual_pid, status)) =
                rustix::process::waitpid(Some(pid.as_pid()), WaitOptions::NOHANG)?
            {
                if actual_pid != pid.as_pid() || status.terminating_signal() != Some(9) {
                    return Err("stolen actual setup child status differs from KILL".into());
                }
                return Ok::<_, Box<dyn std::error::Error + Send + Sync>>(());
            }
            tokio::time::sleep(GROUP_POLL_INTERVAL).await;
        }
    })
    .await??;
    let deadline = cleanup.cleanup_deadline();
    for _ in 0..2 {
        if !matches!(
            cleanup.tick(Instant::now()),
            Err(GroupStopError::NotOwnedChild)
        ) || cleanup.leader_exit_status().is_some()
            || !matches!(cleanup.cleanup_error(), Some(GroupStopError::NotOwnedChild))
            || cleanup.cleanup_deadline() != deadline
        {
            return Err("ECHILD manufactured status/fence or reset cleanup".into());
        }
    }
    if rustix::process::test_kill_process(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::test_kill_process_group(pid.as_pid()) != Err(rustix::io::Errno::SRCH)
        || rustix::process::getpgrp() != parent_group
    {
        return Err("independent stolen-child/parent oracle failed".into());
    }
    rustix::process::test_kill_process_group(parent_group)?;
    eprintln!(
        "SETUP_ECHILD_TEST_ONLY pid={pid:?} stolen=KILL stored=None error=NotOwnedChild original_group=ESRCH process=ESRCH parent={parent_group:?}/alive debt_remains_disqualified"
    );
    Ok(())
}
