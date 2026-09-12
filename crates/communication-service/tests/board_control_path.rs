use communication_client::ControlClient;
use communication_service::{ServiceIdentity, serve_control_connection};
use project_board::*;
use project_board_storage::BoardStore;
use std::sync::Arc;

#[tokio::test]
async fn board_control_roundtrip_preserves_root_thread_and_actor()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "board-control-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(store.clone());
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "board-test", "1").await?;
    let actor = Identity::Human {
        human_id: HumanId::try_from("reader-one".to_owned())?,
    };
    let project_id = ProjectId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let board_id = BoardId::generate();
    client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from("Board".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let topic_id = TopicId::generate();
    client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: actor.clone(),
            acting_for: None,
        })
        .await?;
    let root = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic { topic_id },
            actor: actor.clone(),
            acting_for: None,
            text: MessageText::try_from("Main".to_owned())?,
            references: vec![].try_into()?,
        })
        .await?;
    assert!(root.watch_status.watching);
    let reply = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id.clone(),
            },
            actor: actor.clone(),
            acting_for: None,
            text: MessageText::try_from("Thread".to_owned())?,
            references: vec![ReferenceTarget::Message {
                message_id: root.message.message_id.clone(),
            }]
            .try_into()?,
        })
        .await?;
    assert_eq!(reply.message.actor, actor);
    let thread = client
        .board_thread_show(ThreadShowRequest {
            root_message_id: root.message.message_id,
            reader: None,
        })
        .await?;
    assert_eq!(thread.thread.state, ThreadState::Unresolved);
    drop(client);
    task.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}
