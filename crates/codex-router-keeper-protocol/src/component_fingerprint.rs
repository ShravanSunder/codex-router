use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ComponentFingerprint([u8; 32]);
#[derive(Debug, thiserror::Error)]
pub enum FingerprintError {
    #[error("fingerprint requires exactly 32 bytes or 64 hex characters")]
    Length,
    #[error("fingerprint contains non-hex characters")]
    Hex,
}
impl ComponentFingerprint {
    pub fn from_bytes(value: &[u8]) -> Result<Self, FingerprintError> {
        Ok(Self(
            value.try_into().map_err(|_| FingerprintError::Length)?,
        ))
    }
    pub fn from_hex(value: &str) -> Result<Self, FingerprintError> {
        if value.len() != 64 {
            return Err(FingerprintError::Length);
        }
        let mut bytes = [0; 32];
        for (target, [high, low]) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            *target = digit(*high)? * 16 + digit(*low)?;
        }
        Ok(Self(bytes))
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn to_hex(self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
fn digit(value: u8) -> Result<u8, FingerprintError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(FingerprintError::Hex),
    }
}
impl TryFrom<String> for ComponentFingerprint {
    type Error = FingerprintError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_hex(&value)
    }
}
impl From<ComponentFingerprint> for String {
    fn from(value: ComponentFingerprint) -> Self {
        value.to_hex()
    }
}
