//! Provider credential DTOs with redacted diagnostics.

use std::fmt;

/// Router-owned provider credential bundle.
#[derive(Clone, Eq, PartialEq)]
pub struct RouterCredentialBundle {
    account_id: String,
    access_token: String,
    refresh_token: Option<String>,
    expires_unix_seconds: Option<u64>,
}

impl RouterCredentialBundle {
    /// Creates a credential bundle.
    #[must_use]
    pub fn new(
        account_id: impl Into<String>,
        access_token: impl Into<String>,
        refresh_token: Option<impl Into<String>>,
        expires_unix_seconds: Option<u64>,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            access_token: access_token.into(),
            refresh_token: refresh_token.map(Into::into),
            expires_unix_seconds,
        }
    }

    /// Returns the non-secret account id.
    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Returns the secret access token.
    #[must_use]
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// Returns the secret refresh token if present.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// Returns the access-token expiry timestamp if known.
    #[must_use]
    pub const fn expires_unix_seconds(&self) -> Option<u64> {
        self.expires_unix_seconds
    }
}

impl fmt::Debug for RouterCredentialBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouterCredentialBundle")
            .field("account_id", &self.account_id)
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_token| "<redacted>"),
            )
            .field("expires_unix_seconds", &self.expires_unix_seconds)
            .finish()
    }
}
