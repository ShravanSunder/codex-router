//! Durable identifier newtypes.

use serde::Deserialize;
use serde::Serialize;

use crate::error::IdError;

/// Account identifier used in router-owned state.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct AccountId(String);

impl AccountId {
    /// Builds an account id from a non-empty string.
    pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdError::Empty);
        }

        Ok(Self(value))
    }

    /// Returns the string form for persistence or diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Request identifier used in audit events.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct RequestId(String);

impl RequestId {
    /// Builds a request id.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string form for persistence or diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reservation identifier used by account selection.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct ReservationId(String);

impl ReservationId {
    /// Builds a reservation id.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string form for persistence or diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Previous-response or turn-state affinity key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct AffinityKey(String);

impl AffinityKey {
    /// Builds an affinity key.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string form for persistence or diagnostics.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Local router token generation.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct TokenGeneration(u64);

impl TokenGeneration {
    /// Builds a token generation.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the next generation.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }

    /// Returns the numeric generation.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Router route identifier.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct RouteId(String);
