//! Native bundle identity uses one validated canonical encoding across process boundaries.

use std::{fmt, str::FromStr};

use crate::NativeSchemaBundle;

/// SHA-256 identity of canonical native schema bundle bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct NativeSchemaDigest([u8; 32]);

impl NativeSchemaDigest {
    /// Captures a complete computed digest; every 32-byte value has a valid encoding.
    #[must_use]
    pub fn from_bytes(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<&NativeSchemaBundle> for NativeSchemaDigest {
    fn from(bundle: &NativeSchemaBundle) -> Self {
        Self::from_bytes(*bundle.digest())
    }
}

impl FromStr for NativeSchemaDigest {
    type Err = NativeSchemaDigestError;

    fn from_str(encoded: &str) -> Result<Self, Self::Err> {
        let digits = encoded
            .strip_prefix("sha256:")
            .ok_or(NativeSchemaDigestError::InvalidPrefix)?;
        if digits.len() != 64 {
            return Err(NativeSchemaDigestError::InvalidLength);
        }
        let mut digest = [0_u8; 32];
        let (pairs, _) = digits.as_bytes().as_chunks::<2>();
        for ([high, low], output) in pairs.iter().zip(digest.iter_mut()) {
            *output = (decode_hex_digit(*high)? << 4) | decode_hex_digit(*low)?;
        }
        Ok(Self(digest))
    }
}

impl fmt::Display for NativeSchemaDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("sha256:")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Invalid native digest grammar, without echoing untrusted input.
#[derive(Debug, thiserror::Error)]
pub enum NativeSchemaDigestError {
    #[error("native schema digest must start with sha256:")]
    InvalidPrefix,
    #[error("native schema digest must contain exactly 64 hexadecimal digits")]
    InvalidLength,
    #[error("native schema digest must contain only lowercase hexadecimal digits")]
    InvalidHexDigit,
}

fn decode_hex_digit(digit: u8) -> Result<u8, NativeSchemaDigestError> {
    match digit {
        b'0'..=b'9' => Ok(digit - b'0'),
        b'a'..=b'f' => Ok(digit - b'a' + 10),
        _ => Err(NativeSchemaDigestError::InvalidHexDigit),
    }
}
