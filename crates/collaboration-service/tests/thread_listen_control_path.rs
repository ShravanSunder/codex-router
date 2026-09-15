use collaboration_client::ControlClient;
use collaboration_service::{ServiceIdentity, serve_control_connection};
use message_board::*;
use message_board_storage::BoardStore;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn committed_post_wakes_the_long_poll_control_path_after_debounce()
-> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::temp_dir().join(format!(
        "thread-listen-control-{}.sqlite",
        MessageId::generate().as_str()
    ));
    let store = Arc::new(tokio::sync::Mutex::new(BoardStore::open(&path).await?));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(Arc::clone(&store));
    let (listen_socket, listen_server) = tokio::net::UnixStream::pair()?;
    let listen_service = tokio::spawn(serve_control_connection(listen_server, identity.clone()));
    let (post_socket, post_server) = tokio::net::UnixStream::pair()?;
    let post_service = tokio::spawn(serve_control_connection(post_server, identity));
    let mut listener = ControlClient::initialize(listen_socket, "thread-listener", "1").await?;
    let mut writer = ControlClient::initialize(post_socket, "thread-writer", "1").await?;
    let owner = actor("owner")?;
    let reader = actor("reader")?;
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    writer
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    writer
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from("Board".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    writer
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    let root = writer
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic { topic_id },
            actor: owner,
            acting_for: None,
            text: MessageText::try_from("Root".to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?;
    let listen = listener
        .board_thread_listen(ThreadListenRequest {
            reader,
            selection: ThreadListenSelection::Roots {
                root_message_ids: vec![root.message.message_id.clone()],
            },
            mode: ThreadListenMode::Once {
                max_wait_seconds: 300,
            },
            from_activity_sequence: None,
            acknowledge: false,
        })
        .await?
        .listen;
    let listen_id = listen.listen_id.clone();
    let waiting = tokio::spawn(async move {
        let result = listener
            .board_thread_wait(ThreadWaitRequest { listen_id }, Duration::from_secs(305))
            .await;
        (listener, result)
    });
    tokio::task::yield_now().await;
    let reply = writer
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root.message.message_id,
            },
            actor: actor("writer")?,
            acting_for: None,
            text: MessageText::try_from("Wake the Listen".to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?;
    tokio::task::yield_now().await;
    let (listener, result) = waiting.await?;
    let result = result?;
    let batch_set = result.batch_set.ok_or("Batch set missing")?;
    if batch_set.batches.len() != 1
        || batch_set.batches[0].messages.len() != 1
        || batch_set.batches[0].messages[0].message_id != reply.message.message_id
        || result.end.map(|end| end.reason) != Some(ThreadListenEndReason::Emitted)
    {
        return Err("long-poll Control delivery did not preserve the committed Activity".into());
    }
    listener.close().await?;
    writer.close().await?;
    listen_service.await??;
    post_service.await??;
    drop(store);
    std::fs::remove_file(path)?;
    Ok(())
}

fn actor(value: &str) -> Result<Identity, IdentityValidationError> {
    Ok(Identity::Human {
        human_id: HumanId::try_from(value.to_owned())?,
    })
}
