use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ProviderIdError {
    #[error("provider identity must not be empty")]
    Empty,
}

/// Configuration membership is endpoint policy; this value preserves the exact supplied key.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProviderId(String);
impl ProviderId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderIdError> {
        let value = value.into();
        if value.is_empty() {
            Err(ProviderIdError::Empty)
        } else {
            Ok(Self(value))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl TryFrom<String> for ProviderId {
    type Error = ProviderIdError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<ProviderId> for String {
    fn from(value: ProviderId) -> Self {
        value.0
    }
}
