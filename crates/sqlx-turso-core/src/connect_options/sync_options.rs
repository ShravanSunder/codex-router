//! Turso Sync settings for a connection

use std::{fmt, time::Duration};

/// Turso Sync settings: where to push and pull, and how
///
/// Configured in code only; Sync settings never travel in a connection URL, so the auth token
/// cannot leak through one. `Debug` redacts the token.
#[derive(Clone, Eq, PartialEq)]
pub struct TursoSyncOptions {
    remote_url: String,
    auth_token: Option<String>,
    client_name: Option<String>,
    long_poll_timeout: Option<Duration>,
    bootstrap_if_empty: bool,
}

impl TursoSyncOptions {
    /// Creates Sync settings for a remote base URL, such as `http://127.0.0.1:8080` or
    /// `https://hub.example/db/project-17`
    pub fn new(remote_url: impl Into<String>) -> Self {
        Self {
            remote_url: remote_url.into(),
            auth_token: None,
            client_name: None,
            long_poll_timeout: None,
            bootstrap_if_empty: true,
        }
    }

    /// Returns the remote base URL
    pub fn remote_url(&self) -> &str {
        &self.remote_url
    }

    /// Returns the static auth token sent with Sync requests
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// Returns the client name reported to the Sync server
    pub fn client_name(&self) -> Option<&str> {
        self.client_name.as_deref()
    }

    /// Returns the long-poll timeout for pulls
    pub fn long_poll_timeout(&self) -> Option<Duration> {
        self.long_poll_timeout
    }

    /// Returns whether an empty local database bootstraps from the remote on open
    pub fn bootstrap_if_empty(&self) -> bool {
        self.bootstrap_if_empty
    }

    /// Sets the static auth token sent with Sync requests
    pub fn with_auth_token(mut self, auth_token: impl Into<String>) -> Self {
        self.auth_token = Some(auth_token.into());
        self
    }

    /// Sets the client name reported to the Sync server
    pub fn with_client_name(mut self, client_name: impl Into<String>) -> Self {
        self.client_name = Some(client_name.into());
        self
    }

    /// Sets the long-poll timeout for pulls
    pub fn with_long_poll_timeout(mut self, timeout: Duration) -> Self {
        self.long_poll_timeout = Some(timeout);
        self
    }

    /// Sets whether an empty local database bootstraps from the remote on open
    pub fn with_bootstrap_if_empty(mut self, bootstrap_if_empty: bool) -> Self {
        self.bootstrap_if_empty = bootstrap_if_empty;
        self
    }
}

impl fmt::Debug for TursoSyncOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TursoSyncOptions")
            .field("remote_url", &self.remote_url)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| "<redacted>"),
            )
            .field("client_name", &self.client_name)
            .field("long_poll_timeout", &self.long_poll_timeout)
            .field("bootstrap_if_empty", &self.bootstrap_if_empty)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::TursoSyncOptions;

    #[test]
    fn debug_output_redacts_the_auth_token() {
        // Arrange
        let options = TursoSyncOptions::new("http://127.0.0.1:9").with_auth_token("secret-token");

        // Act
        let rendered = format!("{options:?}");

        // Assert
        assert!(!rendered.contains("secret-token"));
        assert!(rendered.contains("<redacted>"));
    }
}
