use collaboration_client::ControlClient;
use collaboration_client::board::*;
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use message_board_storage::BoardStore;
use std::os::unix::ffi::OsStringExt;
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
    let reader = session_actor("thread-listen-self")?;
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
    for (environment, expected) in [
        (
            "missing",
            "requires exactly one of CODEX_THREAD_ID or CLAUDE_CODE_SESSION_ID",
        ),
        ("both", "is ambiguous"),
        ("invalid", "contain a non-empty session ID"),
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
        command
            .args(["board", "thread", "listen", "--root-message-id"])
            .arg(root_message.message_id.as_str())
            .args(["--once", "--max-wait", "1s", "--actor", "self"])
            .arg("--no-acknowledge")
            .arg("--service-directory")
            .arg(&root)
            .arg("--json")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .kill_on_drop(true);
        match environment {
            "both" => {
                command
                    .env("CODEX_THREAD_ID", "ambiguous-codex")
                    .env("CLAUDE_CODE_SESSION_ID", "ambiguous-claude");
            }
            "invalid" => {
                command.env("CODEX_THREAD_ID", "");
            }
            _ => {}
        }
        let output = command.output().await?;
        if output.status.code() != Some(2)
            || !String::from_utf8_lossy(&output.stdout).contains(expected)
        {
            return Err(format!(
                "--actor self {environment} environment did not fail before mutation: status {:?}, stdout {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
            )
            .into());
        }
    }
    for (environment, codex_thread_id, claude_code_session_id, expected) in [
        (
            "invalid Codex",
            Some(std::ffi::OsString::from_vec(vec![0x80])),
            None,
            "contain a non-empty session ID",
        ),
        (
            "invalid Claude",
            None,
            Some(std::ffi::OsString::from_vec(vec![0x80])),
            "contain a non-empty session ID",
        ),
        (
            "invalid Codex with Claude",
            Some(std::ffi::OsString::from_vec(vec![0x80])),
            Some(std::ffi::OsString::from("ambiguous-claude")),
            "is ambiguous",
        ),
        (
            "Codex with invalid Claude",
            Some(std::ffi::OsString::from("ambiguous-codex")),
            Some(std::ffi::OsString::from_vec(vec![0x80])),
            "is ambiguous",
        ),
        (
            "both invalid",
            Some(std::ffi::OsString::from_vec(vec![0x80])),
            Some(std::ffi::OsString::from_vec(vec![0x81])),
            "is ambiguous",
        ),
    ] {
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
        command
            .args(["board", "thread", "listen", "--root-message-id"])
            .arg(root_message.message_id.as_str())
            .args(["--once", "--max-wait", "1s", "--actor", "self"])
            .arg("--no-acknowledge")
            .arg("--service-directory")
            .arg(&root)
            .arg("--json")
            .env_remove("CODEX_THREAD_ID")
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .kill_on_drop(true);
        if let Some(value) = codex_thread_id {
            command.env("CODEX_THREAD_ID", value);
        }
        if let Some(value) = claude_code_session_id {
            command.env("CLAUDE_CODE_SESSION_ID", value);
        }
        let output = command.output().await?;
        if output.status.code() != Some(2)
            || !String::from_utf8_lossy(&output.stdout).contains(expected)
        {
            return Err(format!(
                "--actor self {environment} environment did not fail before mutation: status {:?}, stdout {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stdout)
            )
            .into());
        }
    }
    if !client
        .board_thread_participant_list(ThreadParticipantListRequest {
            root_message_id: root_message.message_id.clone(),
            page: PageRequest::default(),
        })
        .await?
        .page
        .records
        .is_empty()
    {
        return Err("invalid --actor self attempts mutated Participant state".into());
    }
    let mut listen = tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"));
    listen
        .args(["board", "thread", "join", "--root-message-id"])
        .arg(root_message.message_id.as_str())
        .args(["--actor", "self", "--role", "advisor", "--watch"])
        .args(["--listen", "once", "--max-wait", "2m"])
        .arg("--acknowledge")
        .arg("--service-directory")
        .arg(&root)
        .arg("--json")
        .env("CODEX_THREAD_ID", "thread-listen-self")
        .env_remove("CLAUDE_CODE_SESSION_ID")
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
    let output_lines = String::from_utf8(output.stdout)?;
    let mut output_lines = output_lines.lines();
    let join_result: serde_json::Value = serde_json::from_str(
        output_lines
            .next()
            .ok_or("Join-plus-Listen omitted the committed Join result")?,
    )?;
    if join_result.pointer("/result/participant").is_none() {
        return Err("Join-plus-Listen did not write the Join result before waiting".into());
    }
    let batch_line = output_lines
        .next()
        .ok_or("Join-plus-Listen omitted its Batch set")?;
    if output_lines.next().is_some() {
        return Err("Join-plus-Listen wrote an unexpected extra stdout record".into());
    }
    let batch_set: ThreadListenBatchSet = serde_json::from_str(batch_line)?;
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
        .args(["--once", "--max-wait", "1s", "--actor", "self"])
        .arg("--no-acknowledge")
        .arg("--service-directory")
        .arg(&root)
        .arg("--json")
        .env("CODEX_THREAD_ID", "thread-listen-self")
        .env_remove("CLAUDE_CODE_SESSION_ID")
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

fn session_actor(value: &str) -> Result<Identity, IdentityValidationError> {
    Ok(Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from(SERVICE_ID.to_owned())?,
                endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
            },
            session_id: SessionId::try_from(value.to_owned())?,
        },
    })
}
