//! Failed setup retains the original spawn authority, status and absolute cleanup bound.
use crate::owned_process_group::reap_exact_child;
use crate::{
    GroupStopError,
    lifecycle_bounds::{GROUP_POLL_INTERVAL, GROUP_REAP_BOUND},
};
use codex_router_keeper_protocol::{ChildPgid, ChildPid};
use rustix::{io::Errno, process::Signal};
use std::process::{Child, ExitStatus};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

pub struct OwnedSpawnCleanup {
    child: Option<Child>,
    pid: Option<ChildPid>,
    original_group: Option<ChildPgid>,
    leader_wait: CleanupLeaderWait,
    phase: CleanupPhase,
    last_failure: Option<CleanupFailure>,
    initial_failure: Option<CleanupFailure>,
}
#[derive(Clone, Copy)]
enum CleanupLeaderWait {
    Running,
    Reaped(ExitStatus),
    Disqualified,
}
#[derive(Clone, Copy)]
enum CleanupPhase {
    Unstarted,
    Observing { started: Instant, deadline: Instant },
    Complete,
}
#[derive(Clone, Copy)]
enum CleanupFailure {
    InvalidIdentity,
    NotOwnedChild,
    Wait(Errno),
    Signal(Errno),
    Probe(Errno),
    TimedOut,
    Cancelled,
    TimeWentBackwards,
}
impl CleanupFailure {
    fn error(self) -> GroupStopError {
        match self {
            Self::InvalidIdentity => GroupStopError::InvalidIdentity,
            Self::NotOwnedChild => GroupStopError::NotOwnedChild,
            Self::Wait(error) => GroupStopError::Wait(error),
            Self::Signal(error) => GroupStopError::Signal(error),
            Self::Probe(error) => GroupStopError::Probe(error),
            Self::TimedOut => GroupStopError::CleanupTimedOut,
            Self::Cancelled => GroupStopError::Cancelled,
            Self::TimeWentBackwards => GroupStopError::TimeWentBackwards,
        }
    }
}
impl OwnedSpawnCleanup {
    pub(crate) fn new(child: Child) -> Self {
        let pid = ChildPid::try_from(child.id()).ok();
        Self {
            child: Some(child),
            pid,
            original_group: pid.map(ChildPgid::of_leader),
            leader_wait: CleanupLeaderWait::Running,
            phase: CleanupPhase::Unstarted,
            last_failure: None,
            initial_failure: None,
        }
    }
    pub(crate) fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }
    pub(crate) fn transfer_to_group(&mut self) {
        // std Child has never entered Tokio; dropping it neither waits nor signals.
        drop(self.child.take());
        self.phase = CleanupPhase::Complete;
    }
    pub fn leader_pid(&self) -> Option<ChildPid> {
        self.pid
    }
    pub fn original_process_group_id(&self) -> Option<ChildPgid> {
        self.original_group
    }
    pub fn leader_exit_status(&self) -> Option<ExitStatus> {
        match self.leader_wait {
            CleanupLeaderWait::Reaped(status) => Some(status),
            _ => None,
        }
    }
    pub fn cleanup_deadline(&self) -> Option<Instant> {
        match self.phase {
            CleanupPhase::Observing { deadline, .. } => Some(deadline),
            _ => None,
        }
    }
    /// Last typed cleanup error; a later actual completion fence is a separate fact.
    pub fn cleanup_error(&self) -> Option<GroupStopError> {
        self.last_failure.map(CleanupFailure::error)
    }
    fn fail<T>(&mut self, failure: CleanupFailure) -> Result<T, GroupStopError> {
        self.last_failure = Some(failure);
        Err(failure.error())
    }
    fn reap_leader(&mut self) -> Result<(), GroupStopError> {
        match self.leader_wait {
            CleanupLeaderWait::Reaped(_) => Ok(()),
            CleanupLeaderWait::Disqualified => self.fail(CleanupFailure::NotOwnedChild),
            CleanupLeaderWait::Running => {
                let Some(pid) = self.pid else {
                    return self.fail(CleanupFailure::InvalidIdentity);
                };
                match reap_exact_child(pid) {
                    Ok(Some(status)) => {
                        self.leader_wait = CleanupLeaderWait::Reaped(status);
                        Ok(())
                    }
                    Ok(None) => Ok(()),
                    Err(GroupStopError::NotOwnedChild) => {
                        self.leader_wait = CleanupLeaderWait::Disqualified;
                        self.fail(CleanupFailure::NotOwnedChild)
                    }
                    Err(GroupStopError::Wait(error)) => self.fail(CleanupFailure::Wait(error)),
                    // The shared exact reaper only returns Wait or NotOwnedChild.
                    Err(error) => Err(error),
                }
            }
        }
    }
    pub(crate) fn begin_cleanup(&mut self, now: Instant) {
        if !matches!(self.phase, CleanupPhase::Unstarted) {
            return;
        }
        self.phase = CleanupPhase::Observing {
            started: now,
            deadline: now + GROUP_REAP_BOUND,
        };
        if self.reap_leader().is_err() {
            self.initial_failure = self.last_failure;
            return;
        }
        let (Some(pid), Some(group)) = (self.pid, self.original_group) else {
            self.last_failure = Some(CleanupFailure::InvalidIdentity);
            self.initial_failure = self.last_failure;
            return;
        };
        // This PGID is derived only from our successful process_group(0) spawn.
        // Never use getpgid here: a failed setup may have joined a healthy group.
        let group_signal = rustix::process::kill_process_group(group.as_pid(), Signal::KILL);
        if let Err(error) = group_signal
            && error != Errno::SRCH
        {
            self.last_failure = Some(CleanupFailure::Signal(error));
            self.initial_failure
                .get_or_insert(CleanupFailure::Signal(error));
        }
        if matches!(self.leader_wait, CleanupLeaderWait::Running) {
            let leader_signal = rustix::process::kill_process(pid.as_pid(), Signal::KILL);
            if let Err(error) = leader_signal
                && error != Errno::SRCH
            {
                self.last_failure = Some(CleanupFailure::Signal(error));
                self.initial_failure
                    .get_or_insert(CleanupFailure::Signal(error));
            }
        }
    }
    /// True requires both recorded exact exit and a real original-group ESRCH probe.
    pub fn tick(&mut self, now: Instant) -> Result<bool, GroupStopError> {
        if matches!(self.phase, CleanupPhase::Complete) {
            return Ok(true);
        }
        self.begin_cleanup(now);
        let CleanupPhase::Observing { started, deadline } = self.phase else {
            return self.fail(CleanupFailure::InvalidIdentity);
        };
        if now < started {
            return self.fail(CleanupFailure::TimeWentBackwards);
        }
        self.reap_leader()?;
        let Some(group) = self.original_group else {
            return self.fail(CleanupFailure::InvalidIdentity);
        };
        let empty = match rustix::process::test_kill_process_group(group.as_pid()) {
            Ok(()) => false,
            Err(Errno::SRCH) => true,
            Err(error) => return self.fail(CleanupFailure::Probe(error)),
        };
        if empty && self.leader_exit_status().is_some() {
            self.phase = CleanupPhase::Complete;
            drop(self.child.take());
            return Ok(true);
        }
        if let Some(failure) = incomplete_cleanup_error(self.initial_failure, now, deadline) {
            return self.fail(failure);
        }
        Ok(false)
    }
    /// A cancelled/dropped wait leaves ownership and the first cleanup deadline here.
    pub async fn wait_for_cleanup(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<(), GroupStopError> {
        if matches!(self.phase, CleanupPhase::Complete) {
            return Ok(());
        }
        self.begin_cleanup(Instant::now());
        loop {
            // Constructor has already sent KILL. Waiters observe at the same 20ms
            // cadence within its original absolute bound; ticks remain nonblocking.
            let next_poll = match self.cleanup_deadline() {
                Some(deadline) => std::cmp::min(Instant::now() + GROUP_POLL_INTERVAL, deadline),
                None => Instant::now(),
            };
            tokio::select! { biased;
                _=cancel.cancelled() => return self.fail(CleanupFailure::Cancelled),
                _=tokio::time::sleep_until(next_poll) => {},
            }
            if self.tick(Instant::now())? {
                return Ok(());
            }
        }
    }
}
impl Drop for OwnedSpawnCleanup {
    fn drop(&mut self) {
        if self.child.is_some() {
            // Last resort only. No blocking wait, background reaper or asserted fence.
            self.begin_cleanup(Instant::now());
            let _reap = self.reap_leader();
        }
    }
}

// Called only after the real exit+ESRCH fence has not been observed. An initial
// signal/ownership error stays typed; passing time never converts it to success.
fn incomplete_cleanup_error(
    initial_failure: Option<CleanupFailure>,
    now: Instant,
    deadline: Instant,
) -> Option<CleanupFailure> {
    initial_failure.or_else(|| (now >= deadline).then_some(CleanupFailure::TimedOut))
}
#[cfg(test)]
mod phase_tests {
    use super::*;
    #[test]
    fn incomplete_cleanup_keeps_signal_failure_before_and_after_original_deadline() {
        // Pure supplied-error table, not injected syscall/runtime-error evidence.
        let started = Instant::now();
        let deadline = started + GROUP_REAP_BOUND;
        for now in [started, deadline, deadline + GROUP_POLL_INTERVAL] {
            let error =
                incomplete_cleanup_error(Some(CleanupFailure::Signal(Errno::PERM)), now, deadline);
            assert!(matches!(
                error.map(CleanupFailure::error),
                Some(GroupStopError::Signal(Errno::PERM))
            ));
            let authority =
                incomplete_cleanup_error(Some(CleanupFailure::NotOwnedChild), now, deadline);
            assert!(matches!(
                authority.map(CleanupFailure::error),
                Some(GroupStopError::NotOwnedChild)
            ));
        }
    }
    #[test]
    fn incomplete_cleanup_uses_literal_two_second_deadline_without_reset() {
        assert_eq!(GROUP_REAP_BOUND, std::time::Duration::from_secs(2));
        let started = Instant::now();
        let deadline = started + GROUP_REAP_BOUND;
        assert!(incomplete_cleanup_error(None, deadline - GROUP_POLL_INTERVAL, deadline).is_none());
        for now in [deadline, deadline + GROUP_POLL_INTERVAL] {
            assert!(matches!(
                incomplete_cleanup_error(None, now, deadline).map(CleanupFailure::error),
                Some(GroupStopError::CleanupTimedOut)
            ));
        }
    }
}
