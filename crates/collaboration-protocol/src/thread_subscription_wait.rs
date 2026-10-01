//! Typed Control wire contract for bounded thread subscription waits.
use crate::{MessageText, PushId};
use chrono::{DateTime, Utc};
use message_board::{Identity, MessageId, PendingRootNotice, TopicId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MAX_SUBSCRIPTION_WAIT_SECONDS: u64 = 1500;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ThreadSubscriptionWaitFilter {
    All {},
    Roots { root_message_ids: Vec<MessageId> },
    Topic { topic_id: TopicId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    rename_all = "camelCase",
    deny_unknown_fields,
    try_from = "RawThreadSubscriptionWaitRequest"
)]
pub struct ThreadSubscriptionWaitRequest {
    pub actor: Identity,
    pub filter: ThreadSubscriptionWaitFilter,
    #[schemars(range(max = 1500))]
    pub max_wait_seconds: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawThreadSubscriptionWaitRequest {
    actor: Identity,
    filter: ThreadSubscriptionWaitFilter,
    #[schemars(range(max = 1500))]
    max_wait_seconds: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("maxWaitSeconds must be at most 1500 seconds")]
pub struct InvalidSubscriptionWait;

impl TryFrom<RawThreadSubscriptionWaitRequest> for ThreadSubscriptionWaitRequest {
    type Error = InvalidSubscriptionWait;

    fn try_from(raw: RawThreadSubscriptionWaitRequest) -> Result<Self, Self::Error> {
        let request = Self {
            actor: raw.actor,
            filter: raw.filter,
            max_wait_seconds: raw.max_wait_seconds,
        };
        request.validate()?;
        Ok(request)
    }
}

impl ThreadSubscriptionWaitRequest {
    pub fn validate(&self) -> Result<(), InvalidSubscriptionWait> {
        if self.max_wait_seconds > MAX_SUBSCRIPTION_WAIT_SECONDS {
            return Err(InvalidSubscriptionWait);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SubscriptionWaitBatch {
    Notice {
        push_id: PushId,
        line: MessageText,
        held: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held_since: Option<DateTime<Utc>>,
        draining: bool,
        roots: Vec<PendingRootNotice>,
    },
    Ranges {
        held: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        held_since: Option<DateTime<Utc>>,
        draining: bool,
        roots: Vec<PendingRootNotice>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSubscriptionWaitResult {
    pub batch: Option<SubscriptionWaitBatch>,
}
