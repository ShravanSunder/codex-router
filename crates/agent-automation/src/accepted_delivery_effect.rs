//! The accepted outcome class supplied to storage independently of receipt serialization.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AcceptedDeliveryEffect {
    Started,
    Steered,
    StartedOrSteered,
    Queued,
    PeerMessageWritten,
}
