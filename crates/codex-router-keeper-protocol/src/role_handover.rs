use crate::ComponentKind;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RoleHandoverVersionError {
    #[error("role handover version must be positive")]
    NotPositive,
    #[error("role handover version exceeds u32")]
    Overflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct RoleHandoverVersion(NonZeroU32);

impl RoleHandoverVersion {
    pub fn get(self) -> u32 {
        self.0.get()
    }
}

impl TryFrom<u64> for RoleHandoverVersion {
    type Error = RoleHandoverVersionError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        let value = u32::try_from(value).map_err(|_| RoleHandoverVersionError::Overflow)?;
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(RoleHandoverVersionError::NotPositive)
    }
}

impl From<RoleHandoverVersion> for u64 {
    fn from(value: RoleHandoverVersion) -> Self {
        u64::from(value.get())
    }
}

/// Role-owned, versioned data passed through the keeper without interpreting its body.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleHandover {
    pub role: ComponentKind,
    pub version: RoleHandoverVersion,
    pub body: serde_json::Value,
}
