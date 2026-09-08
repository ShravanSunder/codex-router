//! Eligibility and possible native effects are distinct from reminder timing.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliveryStatus {
    Pending,
    Retryable,
    Dispatching,
    Accepted,
    Discarded,
    Failed,
    Uncertain,
}
impl DeliveryStatus {
    #[must_use]
    pub const fn may_have_native_effect(self) -> bool {
        matches!(self, Self::Dispatching | Self::Accepted | Self::Uncertain)
    }
}
