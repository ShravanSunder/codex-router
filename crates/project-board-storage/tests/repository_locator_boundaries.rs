#![allow(clippy::unwrap_used)]
#[path = "board_behavior_support/mod.rs"]
mod board_behavior_support;
use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;
use sqlx::{Connection, SqliteConnection};

#[tokio::test]
async fn max_bound_escaped_locators_page_fully_below_the_frame_limit() {
    let path = database_path("repository-locator-pages");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let service_id =
        ServiceId::try_from("00000000-0000-4000-8000-000000000001".to_owned()).unwrap();
    for marker in ['a', 'b'] {
        let escaped_path = format!("/{marker}{}", "\\\"".repeat(2_047));
        assert_eq!(escaped_path.len(), 4_096);
        let repository = RepositoryRef::Local {
            service_id: service_id.clone(),
            common_directory: CommonDirectory::try_from(escaped_path).unwrap(),
        };
        store
            .attach_repository(RepositoryAttachRequest {
                project_id: fixture.project_id.clone(),
                repository,
                actor: actor("owner"),
                acting_for: None,
            })
            .await
            .unwrap();
    }

    let mut cursor = None;
    let mut records = Vec::new();
    loop {
        let result = store
            .list_repositories(RepositoryListRequest {
                project_id: fixture.project_id.clone(),
                page: PageRequest {
                    limit: PageLimit::try_from(1).unwrap(),
                    cursor,
                },
            })
            .await
            .unwrap();
        assert!(serde_json::to_vec(&result).unwrap().len() < 1_048_576);
        records.extend(result.page.records);
        cursor = result.page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(records.len(), 2);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn oversized_stored_locator_is_rejected_by_domain_decoding() {
    let path = database_path("repository-locator-invalid-record");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    store.close().await.unwrap();
    let mut connection = SqliteConnection::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let common_directory = format!("/{}", "x".repeat(4_096));
    let repository_key = format!("local:00000000-0000-4000-8000-000000000001:{common_directory}");
    sqlx::query("INSERT INTO project_repositories(project_id,repository_key,kind,origin,service_id,common_directory) VALUES(?,?,'local',NULL,'00000000-0000-4000-8000-000000000001',?)")
        .bind(fixture.project_id.as_str()).bind(repository_key).bind(common_directory).execute(&mut connection).await.unwrap();
    connection.close().await.unwrap();
    let mut store = BoardStore::open(&path).await.unwrap();
    let failure = store
        .list_repositories(RepositoryListRequest {
            project_id: fixture.project_id,
            page: page(10),
        })
        .await
        .unwrap_err();
    assert_eq!(failure.kind, BoardFailureKind::InvalidRecord);
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
