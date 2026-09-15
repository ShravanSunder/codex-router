//! Thread listening contracts use Activity sequence as their only durable ordering cursor.
use crate::{ActivitySequence, Identity, ListenId, MessageId, MessageText};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const THREAD_LISTEN_DEBOUNCE: Duration = Duration::from_secs(30);
pub const THREAD_LISTEN_DEBOUNCE_CAP: Duration = Duration::from_secs(120);
pub const THREAD_LISTEN_POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ThreadListenSelection {
    Watched,
    #[serde(rename_all = "camelCase")]
    Roots {
        root_message_ids: Vec<MessageId>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ThreadListenMode {
    #[serde(rename_all = "camelCase")]
    Once { max_wait_seconds: u64 },
    #[serde(rename_all = "camelCase")]
    Repeating { lifetime_seconds: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenRequest {
    pub reader: Identity,
    pub selection: ThreadListenSelection,
    pub mode: ThreadListenMode,
    pub from_activity_sequence: Option<ActivitySequence>,
    pub acknowledge: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenContext {
    pub reader: Identity,
    pub threads: Vec<ThreadListenThread>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenThread {
    pub root_message_id: MessageId,
    pub initial_delivered_position: ActivitySequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadBatchMessage {
    pub activity_sequence: ActivitySequence,
    pub message_id: MessageId,
    pub actor: Identity,
    pub text: MessageText,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadBatch {
    pub root_message_id: MessageId,
    pub delivered_through: ActivitySequence,
    pub messages: Vec<ThreadBatchMessage>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenBatchSet {
    pub kind: ThreadListenOutputKind,
    pub listen_id: ListenId,
    pub batches: Vec<ThreadBatch>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenOutputKind {
    BatchSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenEndReason {
    Emitted,
    Timeout,
    Lifetime,
    Cancelled,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenEnd {
    pub listen_id: ListenId,
    pub reason: ThreadListenEndReason,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenSnapshot {
    pub listen_id: ListenId,
    pub context: ThreadListenContext,
    pub mode: ThreadListenMode,
    pub acknowledge: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenResult {
    pub listen: ThreadListenSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenShowRequest {
    pub listen_id: ListenId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenShowResult {
    pub listen: ThreadListenSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenCancelRequest {
    pub listen_id: ListenId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenCancelResult {
    pub listen: ThreadListenSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadWaitRequest {
    pub listen_id: ListenId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadWaitResult {
    pub batch_set: Option<ThreadListenBatchSet>,
    pub end: Option<ThreadListenEnd>,
}
