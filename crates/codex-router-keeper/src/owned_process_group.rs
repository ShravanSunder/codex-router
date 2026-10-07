//! Sole exact-PID reaper from birth through exec; no public raw-PID control or Unix receipt.
use crate::{
    GroupStopError, GroupStopProgress, GroupStopStatus, GroupStopTiming,
    group_stop_progress::StopAction,
    lifecycle_bounds::{GROUP_POLL_INTERVAL, GROUP_REAP_BOUND},
};
use codex_router_descriptor_boundary::DescriptorGate;
use codex_router_keeper_protocol::{ChildPgid, ChildPid};
use rustix::process::{Signal, WaitOptions};
use std::{
    os::unix::process::{CommandExt, ExitStatusExt},
    process::{Child as StdChild, ExitStatus},
};
use tokio::{
    process::{ChildStderr, ChildStdin, ChildStdout, Command},
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
pub struct OwnedProcessGroup {
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    pid: ChildPid,
    group: ChildPgid,
    progress: GroupStopProgress,
    leader_wait: LeaderWaitState,
}
impl OwnedProcessGroup {
    pub async fn spawn(command: Command) -> Result<Self, GroupStopError> {
        let mut command = command.into_std();
        command.process_group(0);
        let gate = DescriptorGate::global();
        // Only synchronous spawn runs under the exclusive gate. The temporary
        // owner also contains best-effort cleanup if this launch future is dropped.
        let mut spawned = gate
            .spawn_blocking(move || {
                command
                    .spawn()
                    .map(SpawnedChild::new)
                    .map_err(codex_router_descriptor_boundary::BoundaryError::Io)
            })
            .await?;
        let setup = async {
            let child = spawned
                .child
                .as_mut()
                .ok_or(GroupStopError::InvalidIdentity)?;
            let pid =
                ChildPid::try_from(child.id()).map_err(|_| GroupStopError::InvalidIdentity)?;
            let (stdin, stdout, stderr) = {
                let _creation = gate.creation().await;
                let stdin = child
                    .stdin
                    .take()
                    .map(ChildStdin::from_std)
                    .transpose()
                    .map_err(GroupStopError::Stdio)?;
                let stdout = child
                    .stdout
                    .take()
                    .map(ChildStdout::from_std)
                    .transpose()
                    .map_err(GroupStopError::Stdio)?;
                let stderr = child
                    .stderr
                    .take()
                    .map(ChildStderr::from_std)
                    .transpose()
                    .map_err(GroupStopError::Stdio)?;
                (stdin, stdout, stderr)
            };
            let mut group = Self::adopt_owned_child(pid)?;
            group.stdin = stdin;
            group.stdout = stdout;
            group.stderr = stderr;
            Ok::<_, GroupStopError>(group)
        }
        .await;
        match setup {
            Ok(group) => {
                // Dropping std Child neither kills nor reaps. Only our exact-PID
                // reaper owns the successful child's wait status from now on.
                drop(spawned.child.take());
                Ok(group)
            }
            Err(error) => {
                if let Some(child) = spawned.child.as_mut() {
                    cleanup_failed_spawn(child).await?;
                }
                drop(spawned.child.take());
                Err(error)
            }
        }
    }
    /// Internal per-child ownership check. Full handoff validation must precede
    /// this at the future consumer; a scalar claim by itself grants no authority.
    pub(crate) fn adopt_owned_child(pid: ChildPid) -> Result<Self, GroupStopError> {
        let leader_wait = match reap_exact_child(pid)? {
            Some(status) => LeaderWaitState::Reaped(status),
            None => {
                let group =
                    rustix::process::getpgid(Some(pid.as_pid())).map_err(GroupStopError::Probe)?;
                if group != pid.as_pid() {
                    return Err(GroupStopError::GroupMismatch);
                }
                LeaderWaitState::Running
            }
        };
        Ok(Self {
            stdin: None,
            stdout: None,
            stderr: None,
            pid,
            group: ChildPgid::of_leader(pid),
            progress: GroupStopProgress::Running,
            leader_wait,
        })
    }
    fn reap_leader(&mut self) -> Result<(), GroupStopError> {
        match self.leader_wait {
            LeaderWaitState::Running => match reap_exact_child(self.pid) {
                Ok(Some(status)) => self.leader_wait = LeaderWaitState::Reaped(status),
                Ok(None) => {}
                Err(GroupStopError::NotOwnedChild) => {
                    self.leader_wait = LeaderWaitState::Disqualified;
                    return Err(GroupStopError::NotOwnedChild);
                }
                Err(error) => return Err(error),
            },
            LeaderWaitState::Reaped(_) => {}
            LeaderWaitState::Disqualified => return Err(GroupStopError::NotOwnedChild),
        }
        Ok(())
    }
    pub fn leader_pid(&self) -> ChildPid {
        self.pid
    }
    pub fn process_group_id(&self) -> ChildPgid {
        self.group
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.stderr.take()
    }
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }
    pub fn leader_exit_status(&self) -> Option<ExitStatus> {
        match self.leader_wait {
            LeaderWaitState::Reaped(status) => Some(status),
            LeaderWaitState::Running | LeaderWaitState::Disqualified => None,
        }
    }
    pub fn progress(&self) -> &GroupStopProgress {
        &self.progress
    }
    fn group_exists(&self) -> Result<bool, GroupStopError> {
        if matches!(self.leader_wait, LeaderWaitState::Disqualified) {
            return Err(GroupStopError::NotOwnedChild);
        }
        classify_group_probe(rustix::process::test_kill_process_group(
            self.group.as_pid(),
        ))
    }
    fn send_group_signal(&mut self, signal: Signal) -> Result<bool, GroupStopError> {
        match rustix::process::kill_process_group(self.group.as_pid(), signal) {
            Ok(()) => Ok(true),
            Err(rustix::io::Errno::SRCH) => {
                self.progress.note_empty();
                Ok(false)
            }
            Err(error) => Err(GroupStopError::Signal(error)),
        }
    }
    pub fn begin_stop(
        &mut self,
        timing: GroupStopTiming,
        now: Instant,
    ) -> Result<(), GroupStopError> {
        if let Some(previous) = self.progress.timing() {
            return if previous == timing {
                Ok(())
            } else {
                Err(GroupStopError::StopAlreadyRequested)
            };
        }
        if matches!(self.progress, GroupStopProgress::GroupEmpty { .. }) {
            return Ok(());
        }
        if !self.group_exists()? {
            self.progress.note_empty();
            return Ok(());
        }
        if self.send_group_signal(Signal::TERM)? {
            self.progress = GroupStopProgress::TermSent { at: now, timing };
        }
        Ok(())
    }
    /// Nonblocking event-loop tick; cancellation of a caller's wait never owns this state.
    pub fn tick(&mut self, now: Instant) -> Result<GroupStopStatus, GroupStopError> {
        self.reap_leader()?;
        if matches!(self.progress, GroupStopProgress::GroupEmpty { .. }) {
            return Ok(self.progress.status());
        }
        if !self.group_exists()? {
            self.progress.note_empty();
            return Ok(self.progress.status());
        }
        match self.progress.action(now)? {
            StopAction::SendKill => {
                let timing = self
                    .progress
                    .timing()
                    .ok_or(GroupStopError::StopNotRequested)?;
                if self.send_group_signal(Signal::KILL)? {
                    self.progress = GroupStopProgress::KillSent { at: now, timing };
                }
            }
            StopAction::ObservationExpired => {
                if let GroupStopProgress::KillSent { at, timing } = self.progress {
                    self.progress = GroupStopProgress::TimedOutStillRunning {
                        kill_at: at,
                        timing,
                    };
                }
            }
            StopAction::Poll => {}
        }
        Ok(self.progress.status())
    }
    /// Caller must retain this owner on cancellation/timeout and continue polling it.
    pub async fn wait_for_stop(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<GroupStopStatus, GroupStopError> {
        if matches!(self.progress, GroupStopProgress::Running) {
            return Err(GroupStopError::StopNotRequested);
        }
        loop {
            let status = self.tick(Instant::now())?;
            match status {
                GroupStopStatus::GroupEmpty { .. } if self.leader_exit_status().is_some() => {
                    return Ok(status);
                }
                GroupStopStatus::TimedOutStillRunning
                | GroupStopStatus::ForcedKillObservationExpired => return Ok(status),
                _ => {}
            }
            tokio::select! {biased; _=cancel.cancelled()=>return Err(GroupStopError::Cancelled), _=tokio::time::sleep(GROUP_POLL_INTERVAL)=>{}}
        }
    }
}
impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        // Nonblocking last resort only, never a successful stop/reap fence. Exact
        // owner ticks must finish cleanup; Drop cannot guarantee child/grandchild reap.
        let _reap = self.reap_leader();
        if !matches!(self.progress, GroupStopProgress::GroupEmpty { .. })
            && matches!(self.group_exists(), Ok(true))
        {
            let _signal = rustix::process::kill_process_group(self.group.as_pid(), Signal::KILL);
            let _reap = self.reap_leader();
        }
    }
}

#[derive(Clone, Copy)]
enum LeaderWaitState {
    Running,
    Reaped(ExitStatus),
    Disqualified,
}
fn reap_exact_child(pid: ChildPid) -> Result<Option<ExitStatus>, GroupStopError> {
    match rustix::process::waitpid(Some(pid.as_pid()), WaitOptions::NOHANG) {
        Ok(Some((reaped_pid, status))) if reaped_pid == pid.as_pid() => {
            Ok(Some(ExitStatus::from_raw(status.as_raw())))
        }
        Ok(Some(_)) | Err(rustix::io::Errno::CHILD) => Err(GroupStopError::NotOwnedChild),
        Ok(None) => Ok(None),
        Err(error) => Err(GroupStopError::Wait(error)),
    }
}
struct SpawnedChild {
    child: Option<StdChild>,
}
impl SpawnedChild {
    fn new(child: StdChild) -> Self {
        Self { child: Some(child) }
    }
}
impl Drop for SpawnedChild {
    fn drop(&mut self) {
        // Covers abandoned launch/setup, not a completed stop. No background reaper
        // or blocking wait runs in Drop or under the exclusive spawn gate.
        if let Some(child) = self.child.as_mut()
            && matches!(child.try_wait(), Ok(None))
        {
            let _kill = child.kill();
            let _reap = child.try_wait();
        }
    }
}
async fn cleanup_failed_spawn(child: &mut StdChild) -> Result<(), GroupStopError> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    child.kill()?;
    timeout(GROUP_REAP_BOUND, async {
        loop {
            if child.try_wait()?.is_some() {
                return Ok::<_, GroupStopError>(());
            }
            tokio::time::sleep(GROUP_POLL_INTERVAL).await;
        }
    })
    .await
    .map_err(|_| GroupStopError::CleanupTimedOut)?
}

fn classify_group_probe(result: rustix::io::Result<()>) -> Result<bool, GroupStopError> {
    match result {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::SRCH) => Ok(false),
        Err(error) => Err(GroupStopError::Probe(error)),
    }
}
#[cfg(test)]
mod probe_tests {
    use super::*;
    #[test]
    fn permission_failure_is_not_an_empty_group() {
        assert!(matches!(
            classify_group_probe(Err(rustix::io::Errno::PERM)),
            Err(GroupStopError::Probe(rustix::io::Errno::PERM))
        ));
        assert!(matches!(
            classify_group_probe(Err(rustix::io::Errno::SRCH)),
            Ok(false)
        ));
    }
}

#[cfg(test)]
#[path = "owned_process_group_exec_tests.rs"]
mod exec_tests;
