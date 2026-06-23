//! Previous-response affinity primitives.

use std::fmt;

use hmac::Hmac;
use hmac::Mac;
use serde::Deserialize;
use serde::Serialize;
use sha2::Sha256;
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

const SHA256_HEX_LEN: usize = 64;

/// Raw previous-response id accepted from a local request.
#[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PreviousResponseId(String);

impl PreviousResponseId {
    /// Builds a previous-response id from a non-empty string.
    pub fn new(value: impl Into<String>) -> Result<Self, AffinityPrimitiveError> {
        let value = value.into();
        if value.is_empty() {
            return Err(AffinityPrimitiveError::EmptyPreviousResponseId);
        }

        Ok(Self(value))
    }

    /// Exposes the raw id to the caller for HMAC input only.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PreviousResponseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreviousResponseId([REDACTED])")
    }
}

/// HMAC-SHA256 hash of a previous-response affinity key.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct AffinityKeyHash(String);

impl AffinityKeyHash {
    /// Builds an affinity key hash from a full lowercase SHA-256 hex string.
    pub fn new(value: impl Into<String>) -> Result<Self, AffinityPrimitiveError> {
        let value = value.into();
        if !is_full_lowercase_sha256_hex(&value) {
            return Err(AffinityPrimitiveError::InvalidAffinityKeyHash);
        }

        Ok(Self(value))
    }

    /// Returns the lowercase hex digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AffinityKeyHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Router-owned HMAC secret for previous-response affinity hashes.
#[derive(Clone, Eq, PartialEq)]
pub struct RouterAffinityHashSecret(String);

impl RouterAffinityHashSecret {
    /// Builds a secret from a 32-byte lowercase hex value.
    pub fn new(value: impl Into<String>) -> Result<Self, AffinityPrimitiveError> {
        let value = value.into();
        if !is_full_lowercase_sha256_hex(&value) {
            return Err(AffinityPrimitiveError::InvalidAffinityHashSecret);
        }

        Ok(Self(value))
    }

    /// Exposes the secret material to the caller for HMAC only.
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RouterAffinityHashSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RouterAffinityHashSecret([REDACTED])")
    }
}

/// Computes the durable HMAC hash for a previous-response id.
pub fn hash_previous_response_id(
    secret: &RouterAffinityHashSecret,
    previous_response_id: &PreviousResponseId,
) -> Result<AffinityKeyHash, AffinityPrimitiveError> {
    let mut mac = HmacSha256::new_from_slice(secret.expose_secret().as_bytes())
        .map_err(|_| AffinityPrimitiveError::InvalidAffinityHashSecret)?;
    mac.update(previous_response_id.as_str().as_bytes());
    let digest = mac.finalize().into_bytes();

    AffinityKeyHash::new(lowercase_hex(&digest))
}

/// Affinity primitive validation failure.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum AffinityPrimitiveError {
    /// Previous-response id is empty.
    #[error("previous response id must not be empty")]
    EmptyPreviousResponseId,
    /// Affinity hash must be a full lowercase SHA-256 hex digest.
    #[error("affinity key hash must be 64 lowercase hex chars")]
    InvalidAffinityKeyHash,
    /// Affinity hash secret must be a 32-byte lowercase hex value.
    #[error("affinity hash secret must be 64 lowercase hex chars")]
    InvalidAffinityHashSecret,
}

fn is_full_lowercase_sha256_hex(value: &str) -> bool {
    value.len() == SHA256_HEX_LEN
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    use super::AffinityKeyHash;
    use super::PreviousResponseId;
    use super::RouterAffinityHashSecret;
    use super::hash_previous_response_id;

    #[test]
    fn affinity_hash_uses_full_lowercase_hex() {
        let secret = match RouterAffinityHashSecret::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        ) {
            Ok(secret) => secret,
            Err(error) => panic!("valid secret should parse: {error}"),
        };
        let previous_response_id = match PreviousResponseId::new("resp_123") {
            Ok(previous_response_id) => previous_response_id,
            Err(error) => panic!("valid previous response id should parse: {error}"),
        };

        let hash = match hash_previous_response_id(&secret, &previous_response_id) {
            Ok(hash) => hash,
            Err(error) => panic!("hashing should succeed: {error}"),
        };

        assert_eq!(hash.as_str().len(), 64);
        assert!(
            hash.as_str()
                .bytes()
                .all(|byte| { byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) })
        );
        assert_ne!(hash.as_str(), previous_response_id.as_str());
    }

    #[test]
    fn affinity_primitives_reject_invalid_shapes() {
        assert!(PreviousResponseId::new("").is_err());
        assert!(AffinityKeyHash::new("abc").is_err());
        assert!(
            AffinityKeyHash::new(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeF",
            )
            .is_err()
        );
        assert!(RouterAffinityHashSecret::new("not-hex").is_err());
    }

    #[test]
    fn affinity_secret_and_raw_previous_response_id_do_not_debug_raw_values() {
        let secret_value = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let secret = match RouterAffinityHashSecret::new(secret_value) {
            Ok(secret) => secret,
            Err(error) => panic!("valid secret should parse: {error}"),
        };
        let previous_response_id = match PreviousResponseId::new("resp_secret_canary") {
            Ok(previous_response_id) => previous_response_id,
            Err(error) => panic!("valid previous response id should parse: {error}"),
        };

        assert_eq!(
            format!("{secret:?}"),
            "RouterAffinityHashSecret([REDACTED])"
        );
        assert!(!format!("{secret:?}").contains(secret_value));
        assert_eq!(
            format!("{previous_response_id:?}"),
            "PreviousResponseId([REDACTED])"
        );
        assert!(!format!("{previous_response_id:?}").contains("resp_secret_canary"));
    }
}
