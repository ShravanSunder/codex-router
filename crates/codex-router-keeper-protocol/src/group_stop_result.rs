//! Shared stop-result claims. Only an owned group's observation establishes actual emptiness.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GroupStopResult {
    Graceful,
    Killed,
    TimedOutStillRunning,
    DrainOverrun,
}
