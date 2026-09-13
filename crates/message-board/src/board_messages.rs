use crate::{BoardId, HumanId, Identity, MessageId, MessageText, ProjectId, ThreadState, TopicId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::ops::Deref;

pub const MAX_MESSAGE_REFERENCES: usize = 64;

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("references must contain at most 64 distinct targets")]
pub struct InvalidMessageReferences;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MessageReferences(Vec<ReferenceTarget>);

#[derive(JsonSchema)]
#[schemars(transparent)]
#[allow(dead_code)]
struct MessageReferencesSchema(#[schemars(length(max = 64))] Vec<ReferenceTarget>);

impl JsonSchema for MessageReferences {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MessageReferences".into()
    }
    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        MessageReferencesSchema::json_schema(generator)
    }
}

impl Serialize for MessageReferences {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MessageReferences {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let references = Vec::<ReferenceTarget>::deserialize(deserializer)?;
        Self::try_from(references).map_err(serde::de::Error::custom)
    }
}

impl TryFrom<Vec<ReferenceTarget>> for MessageReferences {
    type Error = InvalidMessageReferences;
    fn try_from(references: Vec<ReferenceTarget>) -> Result<Self, Self::Error> {
        if references.len() > MAX_MESSAGE_REFERENCES
            || references.iter().collect::<HashSet<_>>().len() != references.len()
        {
            return Err(InvalidMessageReferences);
        }
        Ok(Self(references))
    }
}
impl From<MessageReferences> for Vec<ReferenceTarget> {
    fn from(references: MessageReferences) -> Self {
        references.0
    }
}
impl Deref for MessageReferences {
    type Target = [ReferenceTarget];
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl MessageReferences {
    #[must_use]
    pub fn as_slice(&self) -> &[ReferenceTarget] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct ActivitySequence(u64);

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("activity sequence must be between 0 and the maximum SQLite signed integer")]
pub struct InvalidActivitySequence;

impl TryFrom<u64> for ActivitySequence {
    type Error = InvalidActivitySequence;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        (value <= i64::MAX as u64)
            .then_some(Self(value))
            .ok_or(InvalidActivitySequence)
    }
}
impl From<ActivitySequence> for u64 {
    fn from(sequence: ActivitySequence) -> Self {
        sequence.0
    }
}
impl JsonSchema for ActivitySequence {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ActivitySequence".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"integer","minimum":0,"maximum":9223372036854775807_u64})
    }
}
impl ActivitySequence {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ActingForIdentity {
    #[serde(rename_all = "camelCase")]
    Human { human_id: HumanId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Placement {
    #[serde(rename_all = "camelCase")]
    Topic { topic_id: TopicId },
    #[serde(rename_all = "camelCase")]
    Thread { root_message_id: MessageId },
}

#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ReferenceTarget {
    #[serde(rename_all = "camelCase")]
    Message { message_id: MessageId },
    #[serde(rename_all = "camelCase")]
    Thread { root_message_id: MessageId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ReadScope {
    #[serde(rename_all = "camelCase")]
    Topic { topic_id: TopicId },
    #[serde(rename_all = "camelCase")]
    Thread { root_message_id: MessageId },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MessageListScope {
    #[serde(rename_all = "camelCase")]
    Topic {
        topic_id: TopicId,
    },
    #[serde(rename_all = "camelCase")]
    Thread {
        root_message_id: MessageId,
    },
    #[serde(rename_all = "camelCase")]
    Board {
        board_id: BoardId,
    },
    #[serde(rename_all = "camelCase")]
    Project {
        project_id: ProjectId,
    },
    AllProjects,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MessageSelection {
    Latest,
    #[serde(rename_all = "camelCase")]
    AfterPosition {
        after_activity_sequence: ActivitySequence,
    },
    #[serde(rename_all = "camelCase")]
    Range {
        from_activity_sequence: ActivitySequence,
        to_activity_sequence: ActivitySequence,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum MessageSelectionWire {
    Latest,
    #[serde(rename_all = "camelCase")]
    AfterPosition {
        after_activity_sequence: ActivitySequence,
    },
    #[serde(rename_all = "camelCase")]
    Range {
        from_activity_sequence: ActivitySequence,
        to_activity_sequence: ActivitySequence,
    },
}

impl<'de> Deserialize<'de> for MessageSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let selection = match MessageSelectionWire::deserialize(deserializer)? {
            MessageSelectionWire::Latest => Self::Latest,
            MessageSelectionWire::AfterPosition {
                after_activity_sequence,
            } => Self::AfterPosition {
                after_activity_sequence,
            },
            MessageSelectionWire::Range {
                from_activity_sequence,
                to_activity_sequence,
            } => Self::Range {
                from_activity_sequence,
                to_activity_sequence,
            },
        };
        selection.validate().map_err(serde::de::Error::custom)?;
        Ok(selection)
    }
}

impl MessageSelection {
    pub fn validate(&self) -> Result<(), InvalidMessageSelection> {
        match self {
            Self::Range {
                from_activity_sequence,
                to_activity_sequence,
            } if from_activity_sequence > to_activity_sequence => Err(InvalidMessageSelection),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, Eq, PartialEq)]
#[error("range start must not exceed range end")]
pub struct InvalidMessageSelection;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum MessageOrdering {
    NewestFirst,
    OldestFirst,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Message {
    pub message_id: MessageId,
    pub board_id: BoardId,
    pub topic_id: TopicId,
    pub placement: Placement,
    pub actor: Identity,
    pub acting_for: Option<ActingForIdentity>,
    pub text: MessageText,
    pub references: MessageReferences,
    pub activity_sequence: ActivitySequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub root_message_id: MessageId,
    pub state: ThreadState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryRange {
    pub scope: MessageListScope,
    pub selection: MessageSelection,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchStatus {
    pub watching: bool,
    pub starts_after_activity_sequence: Option<ActivitySequence>,
    pub earlier_unwatched_range: Option<HistoryRange>,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InboxActivity {
    #[serde(rename_all = "camelCase")]
    MessageCreated {
        activity_sequence: ActivitySequence,
        acknowledgement_scope: ReadScope,
        message: Message,
    },
    #[serde(rename_all = "camelCase")]
    ThreadStateChanged {
        activity_sequence: ActivitySequence,
        acknowledgement_scope: ReadScope,
        root_message_id: MessageId,
        topic_id: TopicId,
        actor: Identity,
        state: ThreadState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectUnreadSummary {
    pub project_id: ProjectId,
    pub has_unread: bool,
    pub main_tracking_initialized: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bookmark {
    pub reader: Identity,
    pub scope: ReadScope,
    pub through_activity_sequence: ActivitySequence,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum InboxInitializationStatus {
    Existing,
    #[serde(rename_all = "camelCase")]
    Initialized {
        starts_after_activity_sequence: ActivitySequence,
        earlier_history_range: Option<HistoryRange>,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagePage {
    pub scope: MessageListScope,
    pub selection: MessageSelection,
    pub ordering: MessageOrdering,
    pub records: Vec<Message>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxPage {
    pub project_id: ProjectId,
    pub reader: Identity,
    pub read_mode: InboxReadMode,
    pub records: Vec<InboxActivity>,
    pub next_cursor: Option<String>,
    pub initialization: InboxInitializationStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InboxReadMode {
    Unread,
}
