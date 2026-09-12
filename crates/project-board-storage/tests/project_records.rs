#![allow(clippy::unwrap_used)]
use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};

fn actor() -> Identity {
    Identity::Human {
        human_id: HumanId::try_from("owner".to_owned()).unwrap(),
    }
}

#[tokio::test]
async fn project_identity_and_metadata_survive_reopen() {
    let path = std::env::temp_dir().join(format!("board-record-{}.sqlite", uuid::Uuid::now_v7()));
    let project_id = ProjectId::generate();
    let mut store = BoardStore::open(&path).await.unwrap();
    store
        .create_project(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Router".to_owned()).unwrap(),
            description: Description::try_from("Communication".to_owned()).unwrap(),
            actor: actor(),
            acting_for: None,
        })
        .await
        .unwrap();
    store.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    assert_eq!(
        store
            .show_project(ProjectShowRequest {
                project_id: project_id.clone()
            })
            .await
            .unwrap()
            .project
            .project_id,
        project_id
    );
    let renamed = store
        .update_project(ProjectUpdateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Renamed".to_owned()).unwrap(),
            description: Description::try_from("Updated".to_owned()).unwrap(),
            actor: actor(),
            acting_for: None,
        })
        .await
        .unwrap()
        .project;
    assert_eq!(
        store
            .show_project(ProjectShowRequest { project_id })
            .await
            .unwrap()
            .project,
        renamed
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn corrupted_stored_name_is_rejected_by_domain_decoding() {
    let path = std::env::temp_dir().join(format!("board-invalid-{}.sqlite", uuid::Uuid::now_v7()));
    let project_id = ProjectId::generate();
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query!(
        "INSERT INTO board_projects(project_id,name,description) VALUES(?,'','')",
        project_id.as_str(),
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .list_projects(ProjectListRequest {
            repository: None,
            page: PageRequest {
                limit: 100.try_into().unwrap(),
                cursor: None,
            },
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    assert_eq!(
        failure.details,
        BoardErrorDetails::Resource {
            resource: ResourceIdentity::Project { project_id }
        }
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn large_metadata_lists_continue_before_the_control_frame_limit() {
    let path = std::env::temp_dir().join(format!(
        "board-metadata-pages-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let description = "x".repeat(MAX_DESCRIPTION_BYTES);
    let parent_project_id = ProjectId::generate();
    let parent_board_id = BoardId::generate();
    sqlx::query!(
        "INSERT INTO board_projects(project_id,name,description) VALUES(?,?,?)",
        parent_project_id.as_str(),
        "Page parent",
        description,
    )
    .execute(&mut connection)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO project_boards(board_id,project_id,name,description,state) VALUES(?,?,?,?,'active')",
        parent_board_id.as_str(), parent_project_id.as_str(), "Page board parent", description,
    )
    .execute(&mut connection).await.unwrap();
    for index in 0..60 {
        let project_id = ProjectId::generate();
        let board_id = BoardId::generate();
        let topic_id = TopicId::generate();
        let project_name = format!("Large project {index:02}");
        let board_name = format!("Large board {index:02}");
        let topic_name = format!("Large topic {index:02}");
        sqlx::query!(
            "INSERT INTO board_projects(project_id,name,description) VALUES(?,?,?)",
            project_id.as_str(),
            project_name,
            description,
        )
        .execute(&mut connection)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO project_boards(board_id,project_id,name,description,state) VALUES(?,?,?,?,'active')",
            board_id.as_str(), parent_project_id.as_str(), board_name, description,
        )
        .execute(&mut connection).await.unwrap();
        sqlx::query!(
            "INSERT INTO board_topics(topic_id,board_id,name,description) VALUES(?,?,?,?)",
            topic_id.as_str(),
            parent_board_id.as_str(),
            topic_name,
            description,
        )
        .execute(&mut connection)
        .await
        .unwrap();
    }
    connection.close().await.unwrap();

    let mut store = BoardStore::open(&path).await.unwrap();
    let mut project_cursor = None;
    let mut project_count = 0;
    let mut project_pages = 0;
    loop {
        let result = store
            .list_projects(ProjectListRequest {
                repository: None,
                page: PageRequest {
                    limit: 100.try_into().unwrap(),
                    cursor: project_cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        project_count += result.page.records.len();
        project_pages += 1;
        project_cursor = result.page.next_cursor;
        if project_cursor.is_none() {
            break;
        }
    }
    assert_eq!(project_count, 61);
    assert!(project_pages > 1);

    let mut board_cursor = None;
    let mut board_count = 0;
    let mut board_pages = 0;
    loop {
        let result = store
            .list_boards(BoardListRequest {
                project_id: Some(parent_project_id.clone()),
                include_archived: true,
                page: PageRequest {
                    limit: 100.try_into().unwrap(),
                    cursor: board_cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        board_count += result.page.records.len();
        board_pages += 1;
        board_cursor = result.page.next_cursor;
        if board_cursor.is_none() {
            break;
        }
    }
    assert_eq!(board_count, 61);
    assert!(board_pages > 1);

    let mut topic_cursor = None;
    let mut topic_count = 0;
    let mut topic_pages = 0;
    loop {
        let result = store
            .list_topics(TopicListRequest {
                board_id: parent_board_id.clone(),
                page: PageRequest {
                    limit: 100.try_into().unwrap(),
                    cursor: topic_cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        topic_count += result.page.records.len();
        topic_pages += 1;
        topic_cursor = result.page.next_cursor;
        if topic_cursor.is_none() {
            break;
        }
    }
    assert_eq!(topic_count, 60);
    assert!(topic_pages > 1);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn noncanonical_stored_name_whitespace_is_rejected_instead_of_trimmed() {
    let path = std::env::temp_dir().join(format!(
        "board-noncanonical-name-{}.sqlite",
        uuid::Uuid::now_v7()
    ));
    let project_id = ProjectId::generate();
    BoardStore::open(&path)
        .await
        .unwrap()
        .close()
        .await
        .unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    sqlx::query!(
        "INSERT INTO board_projects(project_id,name,description) VALUES(?,' padded ','')",
        project_id.as_str(),
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .show_project(ProjectShowRequest { project_id })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
