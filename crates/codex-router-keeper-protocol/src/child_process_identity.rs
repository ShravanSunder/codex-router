//! Validated identity claims; only a retained spawned Child establishes signal authority.
use serde::{Deserialize, Serialize};
#[derive(Debug, thiserror::Error)]
#[error("child process identity must be representable and greater than one")]
pub struct ChildIdentityError;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct ChildPid(rustix::process::Pid);
impl ChildPid {
    pub fn new(raw: i64) -> Result<Self, ChildIdentityError> {
        if raw <= 1 {
            return Err(ChildIdentityError);
        }
        let signed = i32::try_from(raw).map_err(|_| ChildIdentityError)?;
        rustix::process::Pid::from_raw(signed)
            .map(Self)
            .ok_or(ChildIdentityError)
    }
    pub fn as_pid(self) -> rustix::process::Pid {
        self.0
    }
}
impl TryFrom<i64> for ChildPid {
    type Error = ChildIdentityError;
    fn try_from(raw: i64) -> Result<Self, Self::Error> {
        Self::new(raw)
    }
}
impl TryFrom<u32> for ChildPid {
    type Error = ChildIdentityError;
    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        Self::new(i64::from(raw))
    }
}
impl From<ChildPid> for i64 {
    fn from(pid: ChildPid) -> Self {
        i64::from(pid.0.as_raw_pid())
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "ChildPid", into = "ChildPid")]
pub struct ChildPgid(ChildPid);
impl ChildPgid {
    pub fn of_leader(leader: ChildPid) -> Self {
        Self(leader)
    }
    pub fn as_pid(self) -> rustix::process::Pid {
        self.0.as_pid()
    }
}
impl From<ChildPid> for ChildPgid {
    fn from(leader: ChildPid) -> Self {
        Self::of_leader(leader)
    }
}
impl From<ChildPgid> for ChildPid {
    fn from(group: ChildPgid) -> Self {
        group.0
    }
}
