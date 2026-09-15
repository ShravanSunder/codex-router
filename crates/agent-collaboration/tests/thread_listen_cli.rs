use collaboration_client::ControlClient;
use collaboration_client::board::*;
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use message_board_storage::BoardStore;
use std::os::unix::fs::DirBuilderExt;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";

#[tokio::test]
async fn once_cli_emits_one_batch_then_rearm_times_out_with_exit_three()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "thread-listen-cli-{}",
        MessageId::generate().as_str()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    let database_path = root.join("project-board.sqlite");
    let store = Arc::new(tokio::sync::Mutex::new(
        BoardStore::open(&database_path).await?,
    ));
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, &digest)
        .map_err(std::io::Error::other)?
        .with_board_store(Arc::clone(&store));
    let service = LocalControlService::bind(&root.join("control.sock"), identity)?;
    let manifest = serde_json::from_value(serde_json::json!({
        "version":1,
        "serviceId":SERVICE_ID,
        "serviceEpoch":SERVICE_EPOCH,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
    }))?;
    let publication = ManifestPublication::publish(&root, &manifest)?;
    let stop = CancellationToken::new();
    let service_task = tokio::spawn(service.run(stop.clone()));
    let mut client = ControlClient::connect(&root, "thread-listen-fixture", "1").await?;
    let owner = actor("owner")?;
    let reader = actor("reader")?;
    let writer = actor("writer")?;
    let project_id = ProjectId::generate();
    let board_id = BoardId::generate();
    let topic_id = TopicId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id: project_id.clone(),
            name: ResourceName::try_from("Board".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("Topic".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: owner.clone(),
            acting_for: None,
        })
        .await?;
    let root_message = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic { topic_id },
            actor: owner,
            acting_for: None,
            text: MessageText::try_from("Root".to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?
        .message;
    let reader_json = serde_json::to_string(&reader)?;
    let mut listen = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    listen
        .args(["board", "thread", "listen", "--root-message-id"])
        .arg(root_message.message_id.as_str())
        .args(["--once", "--max-wait", "2m", "--actor"])
        .arg(&reader_json)
        .arg("--acknowledge")
        .arg("--service-directory")
        .arg(&root)
        .arg("--json")
        .kill_on_drop(true);
    let listen_task = tokio::spawn(async move { listen.output().await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let watch = store
                .lock()
                .await
                .show_thread(ThreadShowRequest {
                    root_message_id: root_message.message_id.clone(),
                    reader: Some(reader.clone()),
                })
                .await
                .ok()
                .and_then(|result| result.watch_status);
            if watch.is_some_and(|status| status.watching) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    let reply = client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: root_message.message_id.clone(),
            },
            actor: writer,
            acting_for: None,
            text: MessageText::try_from("Wake the CLI".to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?
        .message;
    let output = listen_task.await??;
    if output.status.code() != Some(0) {
        return Err(format!(
            "Listen exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
        )
        .into());
    }
    let batch_set: ThreadListenBatchSet = serde_json::from_slice(&output.stdout)?;
    if batch_set.kind != ThreadListenOutputKind::BatchSet
        || batch_set.batches.len() != 1
        || batch_set.batches[0].messages.len() != 1
        || batch_set.batches[0].messages[0].message_id != reply.message_id
    {
        return Err("CLI stdout did not contain the committed Batch set".into());
    }
    let unread = client
        .board_inbox_fetch(InboxFetchRequest {
            scope: InboxScope::Project { project_id },
            reader: reader.clone(),
            read_mode: InboxReadMode::Unread,
            page: PageRequest::default(),
        })
        .await?;
    if !unread.page.records.is_empty() {
        return Err(
            "--acknowledge did not advance the emitted Batch's Acknowledged position".into(),
        );
    }

    let timeout = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["board", "thread", "listen", "--root-message-id"])
        .arg(root_message.message_id.as_str())
        .args(["--once", "--max-wait", "1s", "--actor"])
        .arg(reader_json)
        .arg("--no-acknowledge")
        .arg("--service-directory")
        .arg(&root)
        .arg("--json")
        .kill_on_drop(true)
        .output()
        .await?;
    if timeout.status.code() != Some(3) || !timeout.stdout.is_empty() {
        return Err(format!(
            "Re-armed Listen did not exit 3 silently: {:?} {}",
            timeout.status.code(),
            String::from_utf8_lossy(&timeout.stdout)
        )
        .into());
    }

    client.close().await?;
    stop.cancel();
    service_task.await??;
    drop(publication);
    Arc::try_unwrap(store)
        .ok()
        .ok_or("BoardStore still shared")?
        .into_inner()
        .close()
        .await?;
    std::fs::remove_file(database_path)?;
    std::fs::remove_dir(root)?;
    Ok(())
}

fn actor(value: &str) -> Result<Identity, IdentityValidationError> {
    Ok(Identity::Human {
        human_id: HumanId::try_from(value.to_owned())?,
    })
}
