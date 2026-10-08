use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LinkRequestIdError {
    #[error("ProviderLink request identity exhausted")]
    Exhausted,
}

/// Correlation within one link connection; zero is a valid request identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LinkRequestId(u64);
impl LinkRequestId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
    pub fn advance(&mut self) -> Result<Self, LinkRequestIdError> {
        let next = self
            .get()
            .checked_add(1)
            .ok_or(LinkRequestIdError::Exhausted)?;
        *self = Self(next);
        Ok(*self)
    }
}
impl From<u64> for LinkRequestId {
    fn from(value: u64) -> Self {
        Self::new(value)
    }
}
impl From<LinkRequestId> for u64 {
    fn from(value: LinkRequestId) -> Self {
        value.get()
    }
}
