use crate::{
    ActingForIdentity, Board, BoardId, Description, Identity, Page, PageRequest, Project,
    ProjectId, RepositoryRef, ResourceName, Topic, TopicId,
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

contract!(ProjectCreateRequest { project_id: ProjectId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ProjectCreateResult {
    project: Project,
    outcome: String
});
contract!(ProjectUpdateRequest { project_id: ProjectId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(ProjectUpdateResult {
    project: Project,
    outcome: String
});
contract!(ProjectShowRequest {
    project_id: ProjectId
});
contract!(ProjectShowResult { project: Project });
contract!(ProjectListRequest { repository: Option<RepositoryRef>, page: PageRequest });
contract!(ProjectListResult { page: Page<Project> });

contract!(RepositoryAttachRequest { project_id: ProjectId, repository: RepositoryRef, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(RepositoryAttachResult {
    project_id: ProjectId,
    repository: RepositoryRef,
    attached: bool,
    outcome: String
});
contract!(RepositoryDetachRequest { project_id: ProjectId, repository: RepositoryRef, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(RepositoryDetachResult {
    project_id: ProjectId,
    repository: RepositoryRef,
    attached: bool,
    outcome: String
});
contract!(RepositoryListRequest {
    project_id: ProjectId,
    page: PageRequest
});
contract!(RepositoryListResult { page: Page<RepositoryRef> });

contract!(BoardCreateRequest { board_id: BoardId, project_id: ProjectId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(BoardCreateResult {
    board: Board,
    outcome: String
});
contract!(BoardUpdateRequest { board_id: BoardId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(BoardUpdateResult {
    board: Board,
    outcome: String
});
contract!(BoardShowRequest { board_id: BoardId });
contract!(BoardShowResult { board: Board });
contract!(BoardListRequest { project_id: Option<ProjectId>, #[serde(default)] include_archived: bool, page: PageRequest });
contract!(BoardListResult { page: Page<Board> });
contract!(BoardArchiveRequest { board_id: BoardId, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(BoardArchiveResult {
    board: Board,
    outcome: String
});

contract!(TopicCreateRequest { topic_id: TopicId, board_id: BoardId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(TopicCreateResult {
    topic: Topic,
    outcome: String
});
contract!(TopicUpdateRequest { topic_id: TopicId, name: ResourceName, description: Description, actor: Identity, acting_for: Option<ActingForIdentity> });
contract!(TopicUpdateResult {
    topic: Topic,
    outcome: String
});
contract!(TopicListRequest {
    board_id: BoardId,
    page: PageRequest
});
contract!(TopicListResult { page: Page<Topic> });
