use crate::{
    ActingForIdentity, ActivitySequence, Identity, Message, MessageId, MessageListScope,
    MessagePage, MessageReferences, MessageSelection, MessageText, Page, PageRequest, Placement,
    ProjectId, Thread, WatchStatus,
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
contract!(MessageShowResult { message: Message });
contract!(MessageListRequest {
    scope: MessageListScope,
    selection: MessageSelection,
    page: PageRequest
});
contract!(MessageListResult { page: MessagePage });

contract!(ThreadShowRequest { root_message_id: MessageId, reader: Option<Identity> });
contract!(ThreadShowResult { thread: Thread, watch_status: Option<WatchStatus> });
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
contract!(ThreadListRequest {
    project_id: ProjectId,
    reader: Identity,
    #[serde(default)]
    watched_only: bool,
    page: PageRequest
});
contract!(ThreadListResult { page: Page<Thread> });
