use crate::GroupStopError;
use std::time::Duration;
pub const STOP_GRACE: Duration = Duration::from_secs(1);
pub const GROUP_REAP_BOUND: Duration = Duration::from_secs(2);
pub const FORCED_TERM_GRACE: Duration = Duration::from_millis(150);
pub const KILL_OBSERVE_BOUND: Duration = Duration::from_millis(100);
pub const PREPARE_DEADLINE: Duration = Duration::from_secs(30);
pub const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupStopProfile {
    Normal,
    ForcedHandover,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupStopTiming {
    pub(crate) profile: GroupStopProfile,
    pub(crate) term_grace: Duration,
    pub(crate) kill_observe: Duration,
}
impl GroupStopTiming {
    pub fn normal() -> Self {
        Self {
            profile: GroupStopProfile::Normal,
            term_grace: STOP_GRACE,
            kill_observe: GROUP_REAP_BOUND,
        }
    }
    pub fn forced_handover() -> Self {
        Self {
            profile: GroupStopProfile::ForcedHandover,
            term_grace: FORCED_TERM_GRACE,
            kill_observe: KILL_OBSERVE_BOUND,
        }
    }
    pub fn shortened(
        profile: GroupStopProfile,
        term_grace: Duration,
        kill_observe: Duration,
    ) -> Result<Self, GroupStopError> {
        let maximum = match profile {
            GroupStopProfile::Normal => Self::normal(),
            GroupStopProfile::ForcedHandover => Self::forced_handover(),
        };
        if term_grace.is_zero()
            || kill_observe.is_zero()
            || term_grace > maximum.term_grace
            || kill_observe > maximum.kill_observe
        {
            return Err(GroupStopError::InvalidTiming);
        }
        Ok(Self {
            profile,
            term_grace,
            kill_observe,
        })
    }
}
