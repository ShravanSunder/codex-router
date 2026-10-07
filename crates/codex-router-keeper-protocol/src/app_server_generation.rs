//! Generation identities are distinct from collaboration/service epochs.
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use uuid::Uuid;
#[derive(Debug, thiserror::Error)]
pub enum GenerationIdentityError {
    #[error("keeper epoch must be a valid UUID")]
    MalformedEpoch,
    #[error("keeper epoch must not be nil")]
    NilEpoch,
    #[error("generation number must be positive")]
    ZeroNumber,
    #[error("generation number exhausted")]
    NumberExhausted,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct KeeperEpoch(Uuid);
impl KeeperEpoch {
    pub fn fresh() -> Self {
        Self(Uuid::now_v7())
    }
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}
impl TryFrom<String> for KeeperEpoch {
    type Error = GenerationIdentityError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::try_from(
            Uuid::parse_str(&value).map_err(|_| GenerationIdentityError::MalformedEpoch)?,
        )
    }
}
impl TryFrom<Uuid> for KeeperEpoch {
    type Error = GenerationIdentityError;
    fn try_from(value: Uuid) -> Result<Self, Self::Error> {
        if value.is_nil() {
            Err(GenerationIdentityError::NilEpoch)
        } else {
            Ok(Self(value))
        }
    }
}
impl From<KeeperEpoch> for String {
    fn from(value: KeeperEpoch) -> Self {
        value.0.hyphenated().to_string()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct GenerationNumber(NonZeroU64);
impl GenerationNumber {
    pub fn get(self) -> u64 {
        self.0.get()
    }
    /// Exhaustion leaves the current identity intact; only a successful advance changes it.
    pub fn advance(&mut self) -> Result<Self, GenerationIdentityError> {
        let next = self
            .get()
            .checked_add(1)
            .ok_or(GenerationIdentityError::NumberExhausted)?;
        *self = Self::try_from(next)?;
        Ok(*self)
    }
}
impl TryFrom<u64> for GenerationNumber {
    type Error = GenerationIdentityError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        NonZeroU64::new(value)
            .map(Self)
            .ok_or(GenerationIdentityError::ZeroNumber)
    }
}
impl From<GenerationNumber> for u64 {
    fn from(value: GenerationNumber) -> Self {
        value.get()
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationId {
    pub epoch: KeeperEpoch,
    pub number: GenerationNumber,
}
