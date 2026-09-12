#![allow(clippy::unwrap_used)]
mod board_behavior_support;
use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;

#[tokio::test]
async fn cross_project_references_and_unread_summaries_keep_independent_scope() {
    let path = database_path("cross-project");
    let mut store = BoardStore::open(&path).await.unwrap();
    let first = create_fixture(&mut store).await;
    store
        .update_project(ProjectUpdateRequest {
            project_id: first.project_id.clone(),
            name: name("First"),
            description: description("First project"),
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let second = create_fixture(&mut store).await;
    let reader = actor("reader");
    for project_id in [&first.project_id, &second.project_id] {
        store
            .fetch_inbox(InboxFetchRequest {
                project_id: project_id.clone(),
                reader: reader.clone(),
                page: page(50),
            })
            .await
            .unwrap();
    }
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: first.topic_id.clone(),
        },
        actor("first-author"),
        "First project's finding",
        vec![],
    )
    .await;
    let linked = post(
        &mut store,
        Placement::Topic {
            topic_id: second.topic_id.clone(),
        },
        actor("second-author"),
        "Second project's linked finding",
        vec![
            ReferenceTarget::Message {
                message_id: root.message.message_id.clone(),
            },
            ReferenceTarget::Thread {
                root_message_id: root.message.message_id.clone(),
            },
        ],
    )
    .await;
    assert_eq!(linked.message.references.len(), 2);
    let summaries = store
        .list_inbox_projects(InboxProjectsRequest {
            reader: reader.clone(),
            unread_only: true,
            page: page(50),
        })
        .await
        .unwrap();
    assert_eq!(summaries.page.records.len(), 2);
    store
        .acknowledge_inbox(InboxAcknowledgeRequest {
            actor: reader.clone(),
            acting_for: None,
            scope: ReadScope::Topic {
                topic_id: first.topic_id.clone(),
            },
            through_activity_sequence: root.message.activity_sequence,
        })
        .await
        .unwrap();
    let summaries = store
        .list_inbox_projects(InboxProjectsRequest {
            reader: reader.clone(),
            unread_only: true,
            page: page(50),
        })
        .await
        .unwrap();
    assert_eq!(summaries.page.records.len(), 1);
    assert_eq!(summaries.page.records[0].project_id, second.project_id);
    store
        .archive_board(BoardArchiveRequest {
            board_id: first.board_id,
            actor: actor("owner"),
            acting_for: None,
        })
        .await
        .unwrap();
    let thread = post(
        &mut store,
        Placement::Thread {
            root_message_id: linked.message.message_id,
        },
        actor("second-author"),
        "Archived evidence remains referenceable",
        vec![ReferenceTarget::Message {
            message_id: root.message.message_id.clone(),
        }],
    )
    .await;
    assert_eq!(thread.message.references.len(), 1);
    let all = store
        .list_messages(MessageListRequest {
            scope: MessageListScope::AllProjects,
            selection: MessageSelection::Latest,
            page: page(50),
        })
        .await
        .unwrap();
    assert_eq!(all.page.records.len(), 2);
    assert!(all.page.records[0].activity_sequence > all.page.records[1].activity_sequence);
    store.close().await.unwrap();
    let mut reopened = BoardStore::open(&path).await.unwrap();
    assert_eq!(
        reopened
            .show_message(MessageShowRequest {
                message_id: root.message.message_id
            })
            .await
            .unwrap()
            .message
            .text
            .as_str(),
        "First project's finding"
    );
    let summaries = reopened
        .list_inbox_projects(InboxProjectsRequest {
            reader,
            unread_only: true,
            page: page(50),
        })
        .await
        .unwrap();
    assert_eq!(summaries.page.records.len(), 1);
    reopened.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}
