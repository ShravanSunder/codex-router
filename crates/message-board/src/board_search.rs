//! Separate location discovery and immutable-message text search.
use crate::{
    Board, BoardId, Message, MessageListScope, Page, PageRequest, Project, ProjectId, SearchQuery,
    Topic,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DiscoveryScope {
    AllProjects,
    #[serde(rename_all = "camelCase")]
    Project {
        project_id: ProjectId,
    },
    #[serde(rename_all = "camelCase")]
    Board {
        board_id: BoardId,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DiscoveryKind {
    #[default]
    All,
    Project,
    Board,
    Topic,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SearchMessageKind {
    #[default]
    Both,
    TopLevel,
    Thread,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverySearchRequest {
    pub query: SearchQuery,
    pub scope: DiscoveryScope,
    #[serde(default)]
    pub kind: DiscoveryKind,
    #[serde(default)]
    pub include_archived: bool,
    pub page: PageRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum DiscoveryHit {
    Project {
        project: Project,
    },
    Board {
        project: Project,
        board: Board,
    },
    Topic {
        project: Project,
        board: Board,
        topic: Topic,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoverySearchResult {
    pub page: Page<DiscoveryHit>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSearchRequest {
    pub query: SearchQuery,
    pub scope: MessageListScope,
    #[serde(default)]
    pub kind: SearchMessageKind,
    #[serde(default)]
    pub include_archived: bool,
    pub page: PageRequest,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocatedMessage {
    pub project_id: ProjectId,
    pub message: Message,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageSearchResult {
    pub page: Page<LocatedMessage>,
}
