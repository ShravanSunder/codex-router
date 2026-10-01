//! Header sanitization for upstream forwarding.

use codex_router_core::redaction::SecretString;

/// Simple header pair used by protocol tests and adapters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    name: String,
    value: String,
    value_bytes: Vec<u8>,
}

impl Header {
    /// Creates a header.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            name: normalize_header_name(name.into()),
            value_bytes: value.as_bytes().to_vec(),
            value,
        }
    }

    /// Creates a header from validated HTTP header-value bytes.
    pub fn from_bytes(
        name: impl Into<String>,
        value: &[u8],
    ) -> Result<Self, http::header::InvalidHeaderValue> {
        let value = http::HeaderValue::from_bytes(value)?;
        Ok(Self::from_raw_bytes(name, value.as_bytes()))
    }

    /// Preserves bytes from an HTTP header value already validated by the parser.
    pub(crate) fn from_http(name: impl Into<String>, value: &http::HeaderValue) -> Self {
        Self::from_raw_bytes(name, value.as_bytes())
    }

    /// Preserves raw bytes while retaining a lossy UTF-8 view for string-based consumers.
    pub(crate) fn from_raw_bytes(name: impl Into<String>, value: &[u8]) -> Self {
        Self {
            name: normalize_header_name(name.into()),
            value: String::from_utf8_lossy(value).into_owned(),
            value_bytes: value.to_vec(),
        }
    }

    /// Returns normalized header name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns a lossy UTF-8 view of the header value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the exact header-value bytes used for forwarding.
    #[must_use]
    pub fn value_bytes(&self) -> &[u8] {
        &self.value_bytes
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
    chatgpt_account_id: Option<&str>,
) -> HeaderCollection {
    let mut sanitized = headers
        .into_iter()
        .filter(|header| !should_strip_header(header.name()))
        .collect::<Vec<_>>();
    sanitized.push(Header::new(
        "authorization",
        format!("Bearer {}", upstream_auth_token.expose_secret()),
    ));
    if let Some(chatgpt_account_id) = chatgpt_account_id {
        sanitized.push(Header::new("chatgpt-account-id", chatgpt_account_id));
    }

    HeaderCollection::new(sanitized)
}

fn should_strip_header(name: &str) -> bool {
    if name.starts_with("sec-websocket-") {
        return true;
    }

    matches!(
        name,
        "authorization"
            | "chatgpt-account-id"
            | "connection"
            | "content-length"
            | "cookie"
            | "host"
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
