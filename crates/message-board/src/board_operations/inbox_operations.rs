use crate::{
    ActingForIdentity, ActivitySequence, Bookmark, Identity, InboxPage, Page, PageRequest,
    ProjectId, ProjectUnreadSummary, ReadScope,
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

contract!(InboxFetchRequest {
    project_id: ProjectId,
    reader: Identity,
    page: PageRequest
});
contract!(InboxFetchResult { page: InboxPage });
contract!(InboxAcknowledgeRequest { actor: Identity, acting_for: Option<ActingForIdentity>, scope: ReadScope, through_activity_sequence: ActivitySequence });
contract!(InboxAcknowledgeResult {
    bookmark: Bookmark,
    project_unread_summary: ProjectUnreadSummary,
    outcome: String
});
contract!(InboxProjectsRequest {
    reader: Identity,
    #[serde(default)]
    unread_only: bool,
    page: PageRequest
});
contract!(InboxProjectsResult { page: Page<ProjectUnreadSummary> });
