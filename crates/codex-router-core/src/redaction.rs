//! Redaction wrappers for sensitive values.

use std::fmt;

use serde::Serialize;
use serde::Serializer;
use sha2::Digest;
use sha2::Sha256;

use crate::ids::AccountId;

/// String wrapper that redacts accidental display, debug, and serialization.
#[derive(Clone, Eq, PartialEq)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret string.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Explicitly exposes the secret to the caller.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl Serialize for SecretString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str("[REDACTED]")
    }
}

/// Account label safe for default human output, logs, traces, and transcripts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SafeAccountLabel(String);

impl SafeAccountLabel {
    /// Returns the safe display label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SafeAccountLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Builds a safe account label or deterministic redacted tag.
#[must_use]
pub fn safe_account_label(configured_label: &str, account_id: &AccountId) -> SafeAccountLabel {
    if is_safe_configured_label(configured_label, account_id) {
        return SafeAccountLabel(configured_label.to_owned());
    }

    SafeAccountLabel(format!(
        "acct-{}",
        account_id_tag(account_id)
            .chars()
            .take(12)
            .collect::<String>()
    ))
}

/// Returns whether a configured local label is safe for default emission.
#[must_use]
pub fn is_safe_configured_label(configured_label: &str, account_id: &AccountId) -> bool {
    if configured_label.is_empty()
        || configured_label.len() > 64
        || configured_label == account_id.as_str()
        || !configured_label.is_ascii()
    {
        return false;
    }

    let Some(first_byte) = configured_label.bytes().next() else {
        return false;
    };
    if !first_byte.is_ascii_alphanumeric() {
        return false;
    }

    if configured_label.bytes().any(|byte| {
        !byte.is_ascii()
            || byte.is_ascii_control()
            || !(byte.is_ascii_alphanumeric()
                || byte == b'.'
                || byte == b'_'
                || byte == b' '
                || byte == b'-')
    }) {
        return false;
    }

    let lower = configured_label.to_ascii_lowercase();
    ![
        "@",
        "://",
        "/",
        "\\",
        "authorization",
        "bearer",
        "basic",
        "sk-",
        "sess-",
        "oauth",
        "refresh",
        "token",
        "secret",
        "keychain",
        "1password",
    ]
    .iter()
    .any(|forbidden| lower.contains(forbidden))
}

fn account_id_tag(account_id: &AccountId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-router:safe-account-label:v1:");
    hasher.update(account_id.as_str().as_bytes());
    lowercase_hex(&hasher.finalize())
}

fn lowercase_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(hex_digit(byte >> 4));
        output.push(hex_digit(byte & 0x0f));
    }
    output
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'a' + (nibble - 10)),
        _ => unreachable!("nibble is masked to four bits"),
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::AccountId;
    use crate::redaction::is_safe_configured_label;
    use crate::redaction::safe_account_label;

    fn account_id() -> AccountId {
        match AccountId::new("acct_primary") {
            Ok(account_id) => account_id,
            Err(error) => panic!("valid account id should parse: {error}"),
        }
    }

    #[test]
    fn safe_account_label_preserves_simple_local_labels() {
        let account_id = account_id();

        assert_eq!(
            safe_account_label("askluna", &account_id).as_str(),
            "askluna"
        );
        assert_eq!(
            safe_account_label("ssdev profile_1", &account_id).as_str(),
            "ssdev profile_1"
        );
    }

    #[test]
    fn safe_account_label_replaces_unsafe_values_with_deterministic_tag() {
        let account_id = account_id();
        let first = safe_account_label("person@example.com", &account_id);
        let second = safe_account_label("Bearer secret-token", &account_id);

        assert_eq!(first, second);
        assert!(first.as_str().starts_with("acct-"));
        assert_eq!(first.as_str().len(), "acct-".len() + 12);
        assert!(
            first.as_str()["acct-".len()..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert!(!first.as_str().contains("person"));
        assert!(!first.as_str().contains("Bearer"));
    }

    #[test]
    fn configured_label_safety_matches_minimum_predicate() {
        let account_id = account_id();

        for unsafe_label in [
            "",
            " leading",
            "person@example.com",
            "https://example.com/me",
            "folder/name",
            "Authorization header",
            "Bearer abc",
            "sk-test",
            "sess-123",
            "oauth refresh",
            "secret keychain",
            "1password item",
            "acct_primary",
            "nonascii-☃",
        ] {
            assert!(
                !is_safe_configured_label(unsafe_label, &account_id),
                "{unsafe_label} should be unsafe"
            );
        }

        assert!(is_safe_configured_label("matches.profile-1", &account_id));
    }
}
