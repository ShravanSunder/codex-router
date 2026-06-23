//! Authenticated quota fetch facade.

use thiserror::Error;

/// Request for an authenticated quota fetch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaFetchRequest {
    account_label: String,
    route_name: String,
}

impl QuotaFetchRequest {
    /// Creates a quota fetch request.
    #[must_use]
    pub fn new(account_label: impl Into<String>, route_name: impl Into<String>) -> Self {
        Self {
            account_label: account_label.into(),
            route_name: route_name.into(),
        }
    }

    /// Returns the non-secret account label.
    #[must_use]
    pub fn account_label(&self) -> &str {
        &self.account_label
    }

    /// Returns the route name.
    #[must_use]
    pub fn route_name(&self) -> &str {
        &self.route_name
    }
}

/// Response from an authenticated quota fetch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaFetchResponse {
    route_name: String,
    remaining_headroom: u32,
}

impl QuotaFetchResponse {
    /// Creates a quota fetch response.
    #[must_use]
    pub fn new(route_name: impl Into<String>, remaining_headroom: u32) -> Self {
        Self {
            route_name: route_name.into(),
            remaining_headroom,
        }
    }

    /// Returns the route name.
    #[must_use]
    pub fn route_name(&self) -> &str {
        &self.route_name
    }

    /// Returns remaining headroom.
    #[must_use]
    pub const fn remaining_headroom(&self) -> u32 {
        self.remaining_headroom
    }
}

/// Authenticated quota fetch failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AuthenticatedQuotaError {
    /// Auth refresh failed or credentials are unavailable.
    #[error("authenticated quota fetch failed auth: {message}")]
    Auth {
        /// Redacted message.
        message: String,
    },
    /// Provider quota endpoint failed.
    #[error("authenticated quota fetch failed provider: {message}")]
    Provider {
        /// Redacted message.
        message: String,
    },
}

/// Auth-owned facade used by quota refresh.
pub trait AuthenticatedQuotaClient {
    /// Fetches quota using auth-owned credentials.
    fn fetch_quota(
        &self,
        request: QuotaFetchRequest,
    ) -> Result<QuotaFetchResponse, AuthenticatedQuotaError>;
}
