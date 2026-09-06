//! Generation identities reject numeric precision loss across client languages.
use crate::UuidIdentity;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct GenerationNumber(u64);
impl TryFrom<u64> for GenerationNumber {
    type Error = &'static str;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if (1..=9_007_199_254_740_991).contains(&value) {
            Ok(Self(value))
        } else {
            Err("generation must be a positive JSON-safe integer")
        }
    }
}
impl From<GenerationNumber> for u64 {
    fn from(value: GenerationNumber) -> Self {
        value.0
    }
}

/// One managed backend incarnation within a particular service process lifetime.
#[derive(schemars::JsonSchema, Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodexGeneration {
    pub service_epoch: UuidIdentity,
    pub generation: GenerationNumber,
}
