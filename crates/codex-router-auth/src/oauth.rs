//! OpenAI OAuth lifecycle classification.

/// Classification for an OAuth access token at a given time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OAuthTokenStatus {
    /// Token can be used; refresh should start at this timestamp.
    Valid {
        /// Unix second when refresh should begin.
        refresh_after_unix_seconds: u64,
    },
    /// Token is valid but inside the refresh window.
    RefreshNeeded,
    /// Token is expired and must not be used for upstream requests.
    Expired,
}

/// Classification for OpenAI OAuth refresh responses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OAuthRefreshClassification {
    /// Refresh succeeded and returned replacement credentials.
    Succeeded,
    /// Refresh token was rejected by the OpenAI OAuth endpoint.
    RefreshTokenRejected,
    /// Provider asked the router to slow down.
    RateLimited,
    /// Provider failed transiently; retry policy belongs to the caller.
    TransientProviderFailure,
    /// Provider response did not match a known OpenAI OAuth refresh outcome.
    UnexpectedProviderResponse {
        /// HTTP status returned by the provider.
        status: u16,
    },
}

/// Deterministic clock wrapper for token classification tests and workers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TokenClock {
    now_unix_seconds: u64,
}

impl TokenClock {
    /// Creates a clock fixed at a Unix second.
    #[must_use]
    pub const fn new(now_unix_seconds: u64) -> Self {
        Self { now_unix_seconds }
    }

    /// Classifies a token without reading wall-clock time.
    #[must_use]
    pub const fn classify_token(
        self,
        expires_at_unix_seconds: u64,
        refresh_window_seconds: u64,
    ) -> OAuthTokenStatus {
        if expires_at_unix_seconds <= self.now_unix_seconds {
            return OAuthTokenStatus::Expired;
        }

        let refresh_after_unix_seconds =
            expires_at_unix_seconds.saturating_sub(refresh_window_seconds);
        if refresh_after_unix_seconds <= self.now_unix_seconds {
            return OAuthTokenStatus::RefreshNeeded;
        }

        OAuthTokenStatus::Valid {
            refresh_after_unix_seconds,
        }
    }
}

/// Classifies an OpenAI OAuth refresh HTTP response.
#[must_use]
pub fn classify_refresh_response(
    status: u16,
    oauth_error: Option<&str>,
) -> OAuthRefreshClassification {
    match (status, oauth_error) {
        (200, _) => OAuthRefreshClassification::Succeeded,
        (400, Some("invalid_grant")) => OAuthRefreshClassification::RefreshTokenRejected,
        (429, _) => OAuthRefreshClassification::RateLimited,
        (500..=599, _) => OAuthRefreshClassification::TransientProviderFailure,
        _ => OAuthRefreshClassification::UnexpectedProviderResponse { status },
    }
}
