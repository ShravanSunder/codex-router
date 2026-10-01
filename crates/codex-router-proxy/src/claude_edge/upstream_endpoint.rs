//! Claude's fixed production origin and explicitly isolated debug destination.

use thiserror::Error;

/// Claude destination independent of the configurable OpenAI upstream.
///
/// Release builds expose no constructor for a destination override.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeUpstreamEndpoint {
    destination: ClaudeDestination,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ClaudeDestination {
    Production,
    #[cfg(debug_assertions)]
    IsolatedDebug {
        base_url: String,
    },
}

impl Default for ClaudeUpstreamEndpoint {
    fn default() -> Self {
        Self::production()
    }
}

impl ClaudeUpstreamEndpoint {
    /// Uses the fixed Anthropic origin; no caller-supplied URL is accepted.
    #[must_use]
    pub const fn production() -> Self {
        Self {
            destination: ClaudeDestination::Production,
        }
    }

    /// Creates a debug-only override after the caller has required debug isolation.
    /// The CLI/Host owns validation of its isolated runtime directories.
    #[cfg(debug_assertions)]
    pub fn isolated_debug_override(
        base_url: impl Into<String>,
        require_debug_isolation: bool,
    ) -> Result<Self, ClaudeUpstreamEndpointError> {
        if !require_debug_isolation {
            return Err(ClaudeUpstreamEndpointError::DebugIsolationRequired);
        }
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let uri = base_url
            .parse::<http::Uri>()
            .map_err(|_error| ClaudeUpstreamEndpointError::InvalidBaseUrl)?;
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.authority().is_none()
            || uri.query().is_some()
        {
            return Err(ClaudeUpstreamEndpointError::InvalidBaseUrl);
        }
        Ok(Self {
            destination: ClaudeDestination::IsolatedDebug { base_url },
        })
    }

    /// Builds the provider URL without dropping the Messages `/v1` path component.
    #[must_use]
    pub fn url_for_path(&self, request_path: &str) -> String {
        let base_url = match &self.destination {
            ClaudeDestination::Production => "https://api.anthropic.com",
            #[cfg(debug_assertions)]
            ClaudeDestination::IsolatedDebug { base_url } => base_url,
        };
        format!("{base_url}/{}", request_path.trim_start_matches('/'))
    }
}

/// Invalid debug-only destination configuration, without URL or credential material.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ClaudeUpstreamEndpointError {
    /// An override requires the CLI/Host's explicit debug isolation gate.
    #[error("Claude upstream override requires debug isolation")]
    DebugIsolationRequired,
    /// The override must be an absolute HTTP(S) base URL without a query.
    #[error("Claude debug upstream must be an absolute HTTP(S) base URL without a query")]
    InvalidBaseUrl,
}

#[cfg(test)]
mod tests;
