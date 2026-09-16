//! Thread listening contracts use Activity sequence as their only durable ordering cursor.
use crate::{ActivitySequence, Identity, ListenId, MessageId, MessageText};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const THREAD_LISTEN_DEBOUNCE: Duration = Duration::from_secs(5 * 60);
pub const THREAD_LISTEN_DEBOUNCE_CAP: Duration = Duration::from_secs(20 * 60);
pub const THREAD_LISTEN_POLL_INTERVAL: Duration = Duration::from_secs(5);
pub const THREAD_LISTEN_MARK: Duration = Duration::from_secs(25 * 60);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ThreadListenSelection {
    Watched,
    #[serde(rename_all = "camelCase")]
    Roots {
        root_message_ids: Vec<MessageId>,
    },
    #[serde(rename_all = "camelCase")]
    Topic {
        topic_id: crate::TopicId,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenDelivery {
    #[default]
    Stdout,
    Session,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenLifetime {
    Short,
    Long,
}

impl ThreadListenLifetime {
    #[must_use]
    pub const fn seconds(self) -> u64 {
        match self {
            Self::Short => 25 * 60,
            Self::Long => 75 * 60,
        }
    }
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
    #[serde(default)]
    pub delivery: ThreadListenDelivery,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenContext {
    pub reader: Identity,
    pub threads: Vec<ThreadListenThread>,
    pub topic_ids: Vec<crate::TopicId>,
    pub armed_after_sequence: ActivitySequence,
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
    #[serde(default)]
    pub catch_up: bool,
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
pub struct ThreadListenHeartbeat {
    pub kind: ThreadListenHeartbeatKind,
    pub listen_id: ListenId,
    pub last_sequence: Option<ActivitySequence>,
    pub mark: u8,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenHeartbeatKind {
    ListenHeartbeat,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenFinalization {
    pub kind: ThreadListenFinalizationKind,
    pub listen_id: ListenId,
    pub reason: ThreadListenEndReason,
    pub batches_delivered: u64,
    pub first_sequence: Option<ActivitySequence>,
    pub last_sequence: Option<ActivitySequence>,
    pub catch_up: bool,
    pub acknowledged: bool,
    pub last_rejection: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ThreadListenFinalizationKind {
    ListenEnd,
}

#[derive(Clone, Debug)]
pub enum ListenDeliveryRecord {
    Batch(ThreadListenBatchSet),
    Heartbeat(ThreadListenHeartbeat),
    Finalization(ThreadListenFinalization),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BatchSinkFailure {
    Rejected { evidence: serde_json::Value },
    Unavailable,
}

pub trait BatchSink: Send + Sync {
    fn deliver<'a>(
        &'a self,
        record: ListenDeliveryRecord,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), BatchSinkFailure>> + Send + 'a>,
    >;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadListenSnapshot {
    pub listen_id: ListenId,
    pub context: ThreadListenContext,
    pub mode: ThreadListenMode,
    pub acknowledge: bool,
    pub active: bool,
    pub delivery: ThreadListenDelivery,
    pub batches_delivered: u64,
    pub first_sequence: Option<ActivitySequence>,
    pub last_sequence: Option<ActivitySequence>,
    pub catch_up: bool,
    pub acknowledged: bool,
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
