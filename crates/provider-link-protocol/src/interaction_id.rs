use crate::LinkIdentityError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// E11's link-internal interaction key remains stable across E4 replacement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct InteractionId(Uuid);

impl InteractionId {
    pub fn fresh() -> Self {
        Self(Uuid::now_v7())
    }
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}
impl TryFrom<String> for InteractionId {
    type Error = LinkIdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(Uuid::parse_str(&value).map_err(|_| LinkIdentityError::MalformedUuid)?)
    }
}
impl TryFrom<Uuid> for InteractionId {
    type Error = LinkIdentityError;
    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        if value.is_nil() {
            Err(LinkIdentityError::NilUuid)
        } else {
            Ok(Self(value))
        }
    }
}
impl From<InteractionId> for String {
    fn from(value: InteractionId) -> Self {
        value.0.hyphenated().to_string()
    }
}
