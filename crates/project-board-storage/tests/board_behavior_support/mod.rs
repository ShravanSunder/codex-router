#![allow(clippy::unwrap_used)]
use project_board::*;
use project_board_storage::BoardStore;

pub(super) fn name(value: &str) -> ResourceName {
    ResourceName::try_from(value.to_owned()).unwrap()
}
pub(super) fn description(value: &str) -> Description {
    Description::try_from(value.to_owned()).unwrap()
}
pub(super) fn text(value: &str) -> MessageText {
    MessageText::try_from(value.to_owned()).unwrap()
}
pub(super) fn actor(value: &str) -> Identity {
    Identity::Human {
        human_id: HumanId::try_from(value.to_owned()).unwrap(),
    }
}
pub(super) fn page(limit: u32) -> PageRequest {
    PageRequest {
        limit: PageLimit::try_from(limit).unwrap(),
        cursor: None,
    }
}
pub(super) fn no_references() -> MessageReferences {
    MessageReferences::try_from(Vec::new()).unwrap()
}
pub(super) fn database_path(label: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("board-{label}-{}.sqlite", uuid::Uuid::now_v7()))
}

pub(super) struct Fixture {
    pub(super) project_id: ProjectId,
    pub(super) board_id: BoardId,
    pub(super) topic_id: TopicId,
}

pub(super) async fn create_fixture(store: &mut BoardStore) -> Fixture {
    let fixture = Fixture {
        project_id: ProjectId::generate(),
        board_id: BoardId::generate(),
        topic_id: TopicId::generate(),
    };
    create_fixture_with_ids(store, &fixture).await;
    fixture
}

pub(super) async fn create_fixture_with_ids(store: &mut BoardStore, fixture: &Fixture) {
    store
        .create_project(ProjectCreateRequest {
            project_id: fixture.project_id.clone(),
            name: name("Router"),
            description: description("Shared work"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_board(BoardCreateRequest {
            board_id: fixture.board_id.clone(),
            project_id: fixture.project_id.clone(),
            name: name("Engineering"),
            description: description("Coordination"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    store
        .create_topic(TopicCreateRequest {
            topic_id: fixture.topic_id.clone(),
            board_id: fixture.board_id.clone(),
            name: name("Implementation"),
            description: description("Current work"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
}

pub(super) async fn post(
    store: &mut BoardStore,
    placement: Placement,
    author: Identity,
    body: &str,
    references: Vec<ReferenceTarget>,
) -> MessagePostResult {
    store
        .post_message(MessagePostRequest {
            message_id: MessageId::generate(),
            placement,
            actor: author,
            acting_for: None,
            text: text(body),
            references: MessageReferences::try_from(references).unwrap(),
        })
        .await
        .unwrap()
}
