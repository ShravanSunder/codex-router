//! Run occupancy is shared by successor admission and same-Run recovery.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunPhase {
    Waiting,
    Preparing,
    Executing,
    Stopping,
    SummaryRequired,
    SummaryRunning,
    SummaryBlocked,
    Uncertain,
    Finished,
    PreparationFailed,
}

impl RunPhase {
    #[must_use]
    pub const fn occupies_execution(self) -> bool {
        match self {
            Self::Waiting | Self::Finished | Self::PreparationFailed => false,
            Self::Preparing
            | Self::Executing
            | Self::Stopping
            | Self::SummaryRequired
            | Self::SummaryRunning
            | Self::SummaryBlocked
            | Self::Uncertain => true,
        }
    }
}
