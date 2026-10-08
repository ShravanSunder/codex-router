use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum EventSeqError {
    #[error("provider event sequence must be positive")]
    Zero,
    #[error("provider event sequence exhausted")]
    Exhausted,
}

/// Ordered per provider within one E11 incarnation, not per session history.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct EventSeq(NonZeroU64);
impl EventSeq {
    pub const fn get(self) -> u64 {
        self.0.get()
    }
    pub fn advance(&mut self) -> Result<Self, EventSeqError> {
        let next = self.get().checked_add(1).ok_or(EventSeqError::Exhausted)?;
        *self = Self::try_from(next)?;
        Ok(*self)
    }
}
impl TryFrom<u64> for EventSeq {
    type Error = EventSeqError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value).map(Self).ok_or(EventSeqError::Zero)
    }
}
impl From<EventSeq> for u64 {
    fn from(value: EventSeq) -> Self {
        value.get()
    }
}
