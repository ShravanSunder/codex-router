//! Router-assigned identity for one accepted provider Session Input.

use serde::{Deserialize, Serialize};

#[derive(
    schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct InputId(String);

impl InputId {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidInputId> {
        let value = value.into();
        if value.is_empty() || value.len() > 4096 || value.contains('\0') {
            return Err(InvalidInputId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn generate() -> Self {
        Self(uuid::Uuid::now_v7().hyphenated().to_string())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for InputId {
    type Error = InvalidInputId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<InputId> for String {
    fn from(value: InputId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidInputId;

impl std::fmt::Display for InvalidInputId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Input ID must contain 1 to 4096 UTF-8 bytes without NUL")
    }
}

impl std::error::Error for InvalidInputId {}
