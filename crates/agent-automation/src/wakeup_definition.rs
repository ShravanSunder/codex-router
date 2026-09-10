//! Reminder timing and first-fire state are separate from native delivery outcomes.
use crate::{ChangeId, DeliveryId, OccurrenceId, TimingError, TimingRule, WakeupId};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ExpiryRule {
    None,
    At { at: DateTime<Utc> },
    After { seconds: u32 },
}
impl ExpiryRule {
    pub fn resolve(&self, anchor: DateTime<Utc>) -> Result<Option<DateTime<Utc>>, TimingError> {
        match self {
            Self::None => Ok(None),
            Self::At { at } => Ok(Some(*at)),
            Self::After { seconds } => {
                if !(1..=31_536_000).contains(seconds) {
                    return Err(TimingError::InvalidDuration);
                }
                anchor
                    .checked_add_signed(Duration::seconds(i64::from(*seconds)))
                    .map(Some)
                    .ok_or(TimingError::OutOfRange)
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WakeState {
    Active,
    Paused,
    Cancelled,
    Expired,
    Finished,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FirstFire {
    pub wakeup_id: WakeupId,
    pub occurrence_id: OccurrenceId,
    pub due_at_ms: i64,
    pub fired_at_ms: i64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeDefinition<TMessage> {
    pub wakeup_id: WakeupId,
    pub change_id: ChangeId,
    pub message: TMessage,
    pub timing: TimingRule,
    pub anchor_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub created_at_ms: i64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WakeRecord<TMessage> {
    pub definition: WakeDefinition<TMessage>,
    pub state: WakeState,
    pub next_due_at_ms: Option<i64>,
    pub first_fire: Option<FirstFire>,
    pub pending_delivery_id: Option<DeliveryId>,
    pub latest_event_sequence: i64,
}

/// Adapters expose existing message fields without coupling the timing domain to JSON-RPC.
pub trait DurableMessage: Clone + Serialize {
    type Target: Serialize;
    type Content: Serialize;
    type Generation: Serialize;
    fn target(&self) -> &Self::Target;
    fn content(&self) -> &Self::Content;
    fn generation_guard(&self) -> Option<&Self::Generation>;
    fn delivery_mode(&self) -> &'static str;
}
