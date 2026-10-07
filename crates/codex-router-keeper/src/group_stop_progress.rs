//! Pure retained timing; signal success and ESRCH observations come only from the owner.
use crate::{GroupStopError, GroupStopProfile, GroupStopTiming};
use codex_router_keeper_protocol::GroupStopResult;
use tokio::time::Instant;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupStopStatus {
    Running,
    Stopping,
    GroupEmpty { result: GroupStopResult },
    TimedOutStillRunning,
    ForcedKillObservationExpired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupStopProgress {
    Running,
    TermSent {
        at: Instant,
        timing: GroupStopTiming,
    },
    KillSent {
        at: Instant,
        timing: GroupStopTiming,
    },
    TimedOutStillRunning {
        kill_at: Instant,
        timing: GroupStopTiming,
    },
    GroupEmpty {
        result: GroupStopResult,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopAction {
    Poll,
    SendKill,
    ObservationExpired,
}
impl GroupStopProgress {
    pub(crate) fn timing(self) -> Option<GroupStopTiming> {
        match self {
            Self::TermSent { timing, .. }
            | Self::KillSent { timing, .. }
            | Self::TimedOutStillRunning { timing, .. } => Some(timing),
            _ => None,
        }
    }
    pub(crate) fn action(self, now: Instant) -> Result<StopAction, GroupStopError> {
        let (at, bound, action) = match self {
            Self::TermSent { at, timing } => (at, timing.term_grace, StopAction::SendKill),
            Self::KillSent { at, timing } => {
                (at, timing.kill_observe, StopAction::ObservationExpired)
            }
            _ => return Ok(StopAction::Poll),
        };
        let elapsed = now
            .checked_duration_since(at)
            .ok_or(GroupStopError::TimeWentBackwards)?;
        Ok(if elapsed >= bound {
            action
        } else {
            StopAction::Poll
        })
    }
    pub(crate) fn note_empty(&mut self) {
        let result = match self {
            Self::KillSent { .. } | Self::TimedOutStillRunning { .. } => GroupStopResult::Killed,
            Self::GroupEmpty { result } => *result,
            _ => GroupStopResult::Graceful,
        };
        *self = Self::GroupEmpty { result };
    }
    pub(crate) fn status(self) -> GroupStopStatus {
        match self {
            Self::Running => GroupStopStatus::Running,
            Self::GroupEmpty { result } => GroupStopStatus::GroupEmpty { result },
            Self::TimedOutStillRunning { timing, .. } => match timing.profile {
                GroupStopProfile::Normal => GroupStopStatus::TimedOutStillRunning,
                GroupStopProfile::ForcedHandover => GroupStopStatus::ForcedKillObservationExpired,
            },
            _ => GroupStopStatus::Stopping,
        }
    }
}
#[cfg(test)]
#[path = "group_stop_progress_tests.rs"]
mod progress_tests;
