//! Local router bearer-token auth.

use thiserror::Error;

use crate::ids::TokenGeneration;
use crate::redaction::SecretString;

/// Current or historical local router token plus generation metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalRouterTokenRecord {
    token: SecretString,
    generation: TokenGeneration,
}

impl LocalRouterTokenRecord {
    /// Builds a token record.
    #[must_use]
    pub fn new(token: SecretString, generation: TokenGeneration) -> Self {
        Self { token, generation }
    }

    /// Returns the redacted token wrapper.
    #[must_use]
    pub fn token(&self) -> &SecretString {
        &self.token
    }

    /// Returns the token generation.
    #[must_use]
    pub const fn generation(&self) -> TokenGeneration {
        self.generation
    }

    fn matches_presented_token(&self, presented_token: &str) -> bool {
        self.token.expose_secret() == presented_token
    }
}

/// Local auth validation engine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalRouterAuth {
    current: LocalRouterTokenRecord,
    previous: Vec<LocalRouterTokenRecord>,
}

impl LocalRouterAuth {
    /// Builds a local auth validator from the current and previous token records.
    #[must_use]
    pub fn new(current: LocalRouterTokenRecord, previous: Vec<LocalRouterTokenRecord>) -> Self {
        Self { current, previous }
    }

    /// Validates a presented local router bearer token.
    pub fn validate(
        &self,
        presented_token: Option<&str>,
    ) -> Result<TokenGeneration, LocalAuthError> {
        let Some(presented_token) = presented_token else {
            return Err(LocalAuthError::Missing);
        };

        if presented_token.is_empty() {
            return Err(LocalAuthError::Empty);
        }

        if self.current.matches_presented_token(presented_token) {
            return Ok(self.current.generation());
        }

        if self
            .previous
            .iter()
            .any(|record| record.matches_presented_token(presented_token))
        {
            return Err(LocalAuthError::Old);
        }

        Err(LocalAuthError::Wrong)
    }

    /// Returns the current token generation.
    #[must_use]
    pub const fn current_generation(&self) -> TokenGeneration {
        self.current.generation()
    }

    /// Returns whether the supplied generation is still current.
    #[must_use]
    pub fn is_current_generation(&self, generation: TokenGeneration) -> bool {
        self.current.generation() == generation
    }
}

/// Local router auth failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum LocalAuthError {
    /// Header was absent.
    #[error("local router token is missing")]
    Missing,
    /// Header was present but empty.
    #[error("local router token is empty")]
    Empty,
    /// Header used a previously valid generation.
    #[error("local router token is from an old generation")]
    Old,
    /// Header does not match a known token.
    #[error("local router token is invalid")]
    Wrong,
}
