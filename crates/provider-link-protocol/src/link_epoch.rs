use crate::LinkIdentityError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Minted by E4 per ProviderLink connection, separately from session replay epochs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LinkEpoch(Uuid);

impl LinkEpoch {
    pub fn fresh() -> Self {
        Self(Uuid::now_v7())
    }
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}
impl TryFrom<String> for LinkEpoch {
    type Error = LinkIdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(Uuid::parse_str(&value).map_err(|_| LinkIdentityError::MalformedUuid)?)
    }
}
impl TryFrom<Uuid> for LinkEpoch {
    type Error = LinkIdentityError;
    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        if value.is_nil() {
            Err(LinkIdentityError::NilUuid)
        } else {
            Ok(Self(value))
        }
    }
}
impl From<LinkEpoch> for String {
    fn from(value: LinkEpoch) -> Self {
        value.0.hyphenated().to_string()
    }
}
