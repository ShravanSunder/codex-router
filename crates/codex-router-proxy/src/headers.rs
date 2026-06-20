//! Header sanitization for upstream forwarding.

use codex_router_core::redaction::SecretString;

/// Simple header pair used by protocol tests and adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    name: String,
    value: String,
}

impl Header {
    /// Creates a header.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: normalize_header_name(name.into()),
            value: value.into(),
        }
    }

    /// Returns normalized header name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns header value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

/// Header collection preserving input order for forwarded headers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HeaderCollection {
    headers: Vec<Header>,
}

impl HeaderCollection {
    /// Creates a collection.
    #[must_use]
    pub fn new(headers: Vec<Header>) -> Self {
        Self { headers }
    }

    /// Returns first value for a header name.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&str> {
        let normalized = normalize_header_name(name);
        self.headers
            .iter()
            .find(|header| header.name() == normalized)
            .map(Header::value)
    }

    /// Returns all values for a header name.
    #[must_use]
    pub fn values(&self, name: &str) -> Vec<&str> {
        let normalized = normalize_header_name(name);
        self.headers
            .iter()
            .filter(|header| header.name() == normalized)
            .map(Header::value)
            .collect()
    }

    /// Returns all headers.
    #[must_use]
    pub fn as_slice(&self) -> &[Header] {
        &self.headers
    }
}

/// Sanitizes client headers and injects selected upstream auth.
#[must_use]
pub fn sanitize_headers_for_upstream(
    headers: Vec<Header>,
    upstream_auth_token: SecretString,
) -> HeaderCollection {
    let mut sanitized = headers
        .into_iter()
        .filter(|header| !should_strip_header(header.name()))
        .collect::<Vec<_>>();
    sanitized.push(Header::new(
        "authorization",
        format!("Bearer {}", upstream_auth_token.expose_secret()),
    ));

    HeaderCollection::new(sanitized)
}

fn should_strip_header(name: &str) -> bool {
    matches!(
        name,
        "authorization"
            | "connection"
            | "cookie"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "x-codex-router-token"
    )
}

fn normalize_header_name(name: impl AsRef<str>) -> String {
    name.as_ref().to_ascii_lowercase()
}
