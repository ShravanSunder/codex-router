use crate::{
    ActingForIdentity, ActivitySequence, Identity, ImplementerHolder, Message, MessageId,
    MessageListScope, MessagePage, MessageReferences, MessageSelection, MessageText,
    OrchestratorHolder, Page, PageRequest, Participant, ParticipantNote, ParticipantRole,
    Placement, ProjectId, Thread, TopicId, WatchStatus,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! contract {
    ($name:ident { $($(#[$meta:meta])* $field:ident : $ty:ty),* $(,)? }) => {
        #[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        pub struct $name { $($(#[$meta])* pub $field: $ty),* }
    };
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessagePostRequest {
    pub message_id: MessageId,
    pub placement: Placement,
    pub actor: Identity,
    pub acting_for: Option<ActingForIdentity>,
    pub text: MessageText,
    pub references: MessageReferences,
}
contract!(MessagePostResult {
    message: Message,
    watch_status: WatchStatus,
    outcome: String
});
contract!(MessageShowRequest {
    message_id: MessageId
});
/// A single read returns the message itself as the record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct MessageShowResult(pub Message);
contract!(MessageListRequest {
    scope: MessageListScope,
    selection: MessageSelection,
    page: PageRequest
});
contract!(MessageListResult { page: MessagePage });

contract!(ThreadShowRequest { root_message_id: MessageId, reader: Option<Identity> });
/// A single read returns the thread itself as the record, carrying the reader's
/// watch status alongside the thread's own fields.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ThreadShowResult {
    #[serde(flatten)]
    pub thread: Thread,
    pub watch_status: Option<WatchStatus>,
}
contract!(ThreadResolveRequest { root_message_id: MessageId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ThreadResolveResult { thread: Thread, activity_sequence: Option<ActivitySequence>, outcome: String });
contract!(ThreadUnresolveRequest { root_message_id: MessageId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ThreadUnresolveResult { thread: Thread, activity_sequence: Option<ActivitySequence>, outcome: String });
contract!(ThreadWatchRequest { root_message_id: MessageId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ThreadWatchResult {
    thread: Thread,
    watch_status: WatchStatus,
    outcome: String
});
contract!(ThreadUnwatchRequest { root_message_id: MessageId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ThreadUnwatchResult {
    thread: Thread,
    watch_status: WatchStatus,
    outcome: String
});
contract!(TopicWatchRequest { topic_id: TopicId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(TopicWatchResult {
    topic_id: TopicId,
    watching: bool,
    starts_after_activity_sequence: Option<ActivitySequence>,
    outcome: String
});
contract!(ThreadListRequest {
    project_id: ProjectId,
    reader: Identity,
    #[serde(default)]
    watched_only: bool,
    page: PageRequest
});
contract!(ThreadListResult { page: Page<Thread> });

contract!(ThreadCreateRequest {
    message_id: MessageId,
    topic_id: TopicId,
    actor: Identity,
    acting_for: Option<ActingForIdentity>,
    text: MessageText,
    references: MessageReferences,
    role: Option<ParticipantRole>,
    watch: bool
});

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ThreadCreatorParticipation {
    #[serde(rename_all = "camelCase")]
    Joined {
        participant: Box<Participant>,
    },
    NotJoined,
}

contract!(ThreadCreateResult {
    message: Message,
    creator_participation: ThreadCreatorParticipation,
    orchestrator: Option<OrchestratorHolder>,
    implementer: Option<ImplementerHolder>,
    watch_status: WatchStatus,
    outcome: String
});
contract!(ThreadJoinRequest {
    root_message_id: MessageId,
    actor: Identity,
    role: ParticipantRole,
    watch: bool,
    replace: Option<Identity>,
    note: Option<ParticipantNote>
});
contract!(ThreadJoinResult {
    participant: Participant,
    orchestrator: Option<OrchestratorHolder>,
    implementer: Option<ImplementerHolder>,
    watch_status: WatchStatus,
    outcome: String
});
contract!(ThreadLeaveRequest {
    root_message_id: MessageId,
    actor: Identity,
    to: Option<Identity>,
    resolve: bool
});
contract!(ThreadLeaveResult {
    participant: Participant,
    thread: Thread,
    orchestrator: Option<OrchestratorHolder>,
    implementer: Option<ImplementerHolder>,
    watch_status: WatchStatus,
    outcome: String
});
contract!(ThreadParticipantListRequest {
    root_message_id: MessageId,
    page: PageRequest
});
contract!(ThreadParticipantListResult {
    page: Page<Participant>,
    orchestrator: Option<OrchestratorHolder>,
    implementer: Option<ImplementerHolder>
});
