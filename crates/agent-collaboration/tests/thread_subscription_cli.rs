use collaboration_client::ControlClient;
use collaboration_client::board::*;
use collaboration_service::{
    BoardAvailability, LocalControlService, MachineIdentity, ManifestPublication, ServiceIdentity,
    SessionDeliveryRouter, SessionMessageDelivery, SubscriptionDeliveryService,
    SubscriptionDeliveryServiceProps, SystemSubscriptionClock, TargetPresenceProbe,
};
use message_board_storage::BoardStore;
use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[path = "thread_subscription_cli/wait_uncertainty.rs"]
mod wait_uncertainty;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000101";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000102";
const SESSION_ID: &str = "thread-subscription-cli";
const NO_WATCH_SESSION_ID: &str = "thread-subscription-no-watch";
type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;

#[tokio::test]
async fn subscribe_join_wait_and_cancel_use_real_control_and_sqlite_paths() -> TestResult {
    let directory = tempfile::tempdir_in("/tmp")?;
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))?;
    let service_directory = directory.path();
    let database_path = service_directory.join("project-board.sqlite");
    let automation_path = service_directory.join("automation.sqlite");
    let store = Arc::new(tokio::sync::Mutex::new(
        BoardStore::open(&database_path).await?,
    ));
    let push_store = Arc::new(tokio::sync::Mutex::new(
        automation_storage::AutomationStore::open(&automation_path).await?,
    ));
    let digest = format!("sha256:{}", "a".repeat(64));
    let router = Arc::new(SessionDeliveryRouter::new(Vec::new()));
    let delivery: Arc<dyn SessionMessageDelivery> = router.clone();
    let presence: Arc<dyn TargetPresenceProbe> = router.clone();
    let machine_identity = MachineIdentity::new(
        collaboration_protocol::UuidIdentity::try_from(SERVICE_ID.to_owned())?,
        None,
    )?;
    let subscription_delivery =
        SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Available(Arc::clone(&store)),
            push_store: Arc::clone(&push_store),
            delivery,
            presence: Arc::clone(&presence),
            machine_identity,
            clock: Arc::new(SystemSubscriptionClock),
        });
    subscription_delivery.start().await?;
    let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
        .map_err(std::io::Error::other)?
        .with_board_store(Arc::clone(&store))
        .with_session_delivery(router)
        .with_subscription_delivery_service(subscription_delivery.clone(), presence);
    let service = LocalControlService::bind(&service_directory.join("control.sock"), identity)?;
    let manifest = serde_json::from_value(serde_json::json!({
        "version":2,
        "serviceId":SERVICE_ID,
        "serviceEpoch":SERVICE_EPOCH,
        "machineLabel":"fixture-host",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"},
    }))?;
    let publication = ManifestPublication::publish(service_directory, &manifest)?;
    let stop = CancellationToken::new();
    let service_task = tokio::spawn(service.run(stop.clone()));
    let mut client =
        ControlClient::connect(service_directory, "thread-subscription-cli-test", "1").await?;

    let owner = human_actor("owner")?;
    let reader = session_actor(SESSION_ID)?;
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
            project_id,
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
    let first_root = post_root(&mut client, &owner, &topic_id, "first root").await?;

    let join = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "join",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--role",
            "advisor",
            "--mode",
            "poll",
            "--when-idle",
            "drop",
        ],
    )
    .await?;
    ensure_cli_success(&join)?;
    let first_watch = client
        .board_thread_show(ThreadShowRequest {
            root_message_id: first_root.clone(),
            reader: Some(reader.clone()),
        })
        .await?
        .watch_status
        .ok_or("joined session Watch status was omitted")?;
    ensure(
        first_watch.watching,
        "join without --watch must watch by default",
    )?;
    let joined_subscriptions = client
        .board_thread_subscriptions(collaboration_client::protocol::ThreadSubscriptionsRequest {
            actor: reader.clone(),
        })
        .await?;
    ensure(
        joined_subscriptions.subscriptions.len() == 1,
        "joining did not create exactly one subscription",
    )?;
    let joined_subscription = joined_subscriptions
        .subscriptions
        .first()
        .ok_or("joining did not create a subscription")?;
    ensure(
        joined_subscription.policy.mode() == SubscriptionMode::Poll,
        "join --mode was not applied",
    )?;
    ensure(
        joined_subscription.policy.when_idle() == WhenIdle::Drop,
        "join --when-idle was not applied",
    )?;

    let no_watch_actor = serde_json::to_string(&session_actor(NO_WATCH_SESSION_ID)?)?;
    let no_watch_join = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "join",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            no_watch_actor.as_str(),
            "--role",
            "participant",
            "--no-watch",
        ],
    )
    .await?;
    ensure_cli_success(&no_watch_join)?;
    let no_watch_status = client
        .board_thread_show(ThreadShowRequest {
            root_message_id: first_root.clone(),
            reader: Some(session_actor(NO_WATCH_SESSION_ID)?),
        })
        .await?
        .watch_status
        .ok_or("joined session Watch status was omitted")?;
    ensure(
        !no_watch_status.watching,
        "--no-watch must opt out of the Watch",
    )?;
    ensure(
        client
            .board_thread_subscriptions(
                collaboration_client::protocol::ThreadSubscriptionsRequest {
                    actor: session_actor(NO_WATCH_SESSION_ID)?,
                },
            )
            .await?
            .subscriptions
            .is_empty(),
        "--no-watch join created a subscription",
    )?;

    let thread_subscribe = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "subscribe",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--mode",
            "deliver",
            "--when-idle",
            "hold",
            "--quiet",
            "0s",
            "--cap",
            "5m",
            "--for",
            "1h",
        ],
    )
    .await?;
    let thread_view = parse_cli_result(&thread_subscribe)?;
    ensure(
        thread_view["scope"]["kind"] == "thread",
        "thread subscribe returned a different scope kind",
    )?;
    ensure(
        thread_view["scope"]["rootMessageId"] == first_root.as_str(),
        "thread subscribe returned a different root",
    )?;
    ensure(
        thread_view["policy"]["mode"] == "deliver",
        "thread subscribe mode was not applied",
    )?;
    ensure(
        thread_view["policy"]["whenIdle"] == "hold",
        "thread subscribe when-idle policy was not applied",
    )?;
    ensure(
        thread_view["policy"]["timing"]["quietSeconds"] == 0,
        "thread subscribe quiet period was not applied",
    )?;
    ensure(
        thread_view["policy"]["timing"]["capSeconds"] == 300,
        "thread subscribe cap was not applied",
    )?;
    ensure(
        thread_view["policy"]["lifetime"] == 3600,
        "thread subscribe lifetime was not applied",
    )?;

    let topic_subscribe = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "subscribe",
            "--topic-id",
            topic_id.as_str(),
            "--actor",
            "self",
            "--mode",
            "off",
        ],
    )
    .await?;
    let topic_view = parse_cli_result(&topic_subscribe)?;
    ensure(
        topic_view["scope"]["kind"] == "topic",
        "topic subscribe returned a different scope kind",
    )?;
    ensure(
        topic_view["scope"]["topicId"] == topic_id.as_str(),
        "topic subscribe returned a different topic",
    )?;

    let list = run_cli(
        service_directory,
        &["board", "thread", "subscriptions", "--actor", "self"],
    )
    .await?;
    let list_result = parse_cli_result(&list)?;
    let subscriptions = list_result["subscriptions"]
        .as_array()
        .ok_or("subscriptions command omitted its subscription list")?;
    ensure(
        subscriptions.len() == 2,
        "subscriptions command returned the wrong number of records",
    )?;
    ensure(
        subscriptions.iter().any(|subscription| {
            subscription["scope"]["kind"] == "thread"
                && subscription["scope"]["rootMessageId"] == first_root.as_str()
        }),
        "subscriptions command omitted the Thread scope",
    )?;
    ensure(
        subscriptions.iter().any(|subscription| {
            subscription["scope"]["kind"] == "topic"
                && subscription["scope"]["topicId"] == topic_id.as_str()
        }),
        "subscriptions command omitted the Topic scope",
    )?;

    let non_poll_wait = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "wait",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--max-wait",
            "1s",
        ],
    )
    .await?;
    ensure(
        non_poll_wait.status.code() == Some(4),
        "wait against a deliver-mode subscription did not return a refusal",
    )?;
    let non_poll_error: Value = serde_json::from_slice(&non_poll_wait.stdout)?;
    let non_poll_message = non_poll_error["error"]["message"]
        .as_str()
        .ok_or("non-poll wait refusal omitted its message")?;
    ensure(
        non_poll_message.contains("wait requires a poll subscription")
            && non_poll_message.contains("board thread subscribe --mode poll"),
        "non-poll wait refusal omitted the poll-mode correction",
    )?;

    let poll_subscription = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "subscribe",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--mode",
            "poll",
            "--quiet",
            "0s",
            "--cap",
            "0s",
        ],
    )
    .await?;
    parse_cli_result(&poll_subscription)?;

    let timeout_wait = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "wait",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--max-wait",
            "0s",
        ],
    )
    .await?;
    let timeout_result = parse_cli_result(&timeout_wait)?;
    ensure(
        timeout_result["batch"].is_null(),
        "wait with no pending activity did not return an empty batch",
    )?;

    client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Thread {
                root_message_id: first_root.clone(),
            },
            actor: session_actor(NO_WATCH_SESSION_ID)?,
            acting_for: None,
            text: MessageText::try_from("poll wait due".to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?;
    let due_wait = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "wait",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--max-wait",
            "5s",
        ],
    )
    .await?;
    let due_result = parse_cli_result(&due_wait)?;
    ensure(
        due_result["batch"]["kind"] == "notice",
        "due poll wait did not return the session Notice variant",
    )?;
    let due_roots = due_result["batch"]["roots"]
        .as_array()
        .ok_or("due poll Notice omitted root ranges")?;
    ensure(
        due_roots.len() == 1,
        "due poll wait returned the wrong number of root ranges",
    )?;
    ensure(
        due_roots[0]["rootId"] == first_root.as_str(),
        "due poll wait returned a different root range",
    )?;
    let repeated_wait = run_cli(
        service_directory,
        &[
            "board",
            "thread",
            "wait",
            "--root-message-id",
            first_root.as_str(),
            "--actor",
            "self",
            "--max-wait",
            "0s",
        ],
    )
    .await?;
    ensure(
        parse_cli_result(&repeated_wait)?["batch"].is_null(),
        "the handed-off poll batch was returned a second time",
    )?;

    for (scope_flag, scope_id) in [
        ("--root-message-id", first_root.as_str()),
        ("--topic-id", topic_id.as_str()),
    ] {
        let unsubscribe = run_cli(
            service_directory,
            &[
                "board",
                "thread",
                "unsubscribe",
                scope_flag,
                scope_id,
                "--actor",
                "self",
            ],
        )
        .await?;
        let ended_view = parse_cli_result(&unsubscribe)?;
        ensure(
            ended_view["state"] == "ended",
            "unsubscribe did not return an ended subscription",
        )?;
        ensure(
            ended_view["endReason"]["kind"] == "cancelled",
            "unsubscribe did not report the cancelled end reason",
        )?;
    }
    ensure(
        client
            .board_thread_subscriptions(
                collaboration_client::protocol::ThreadSubscriptionsRequest { actor: reader },
            )
            .await?
            .subscriptions
            .is_empty(),
        "subscriptions remained after cancelling every scope",
    )?;

    client.close().await?;
    stop.cancel();
    service_task.await??;
    drop(publication);
    subscription_delivery.shutdown().await;
    drop(subscription_delivery);
    Arc::try_unwrap(store)
        .ok()
        .ok_or("BoardStore still shared")?
        .into_inner()
        .close()
        .await?;
    Arc::try_unwrap(push_store)
        .ok()
        .ok_or("AutomationStore still shared")?
        .into_inner()
        .close()
        .await?;
    Ok(())
}

async fn post_root(
    client: &mut ControlClient,
    owner: &Identity,
    topic_id: &TopicId,
    text: &str,
) -> Result<MessageId, TestError> {
    Ok(client
        .board_message_post(MessagePostRequest {
            message_id: MessageId::generate(),
            placement: Placement::Topic {
                topic_id: topic_id.clone(),
            },
            actor: owner.clone(),
            acting_for: None,
            text: MessageText::try_from(text.to_owned())?,
            references: Vec::new().try_into()?,
        })
        .await?
        .message
        .message_id)
}

async fn run_cli(
    service_directory: &Path,
    arguments: &[&str],
) -> Result<std::process::Output, TestError> {
    Ok(tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(arguments)
            .arg("--service-directory")
            .arg(service_directory)
            .arg("--json")
            .env("CODEX_THREAD_ID", SESSION_ID)
            .env_remove("CLAUDE_CODE_SESSION_ID")
            .env_remove("CURSOR_CONVERSATION_ID")
            .kill_on_drop(true)
            .output(),
    )
    .await??)
}

fn ensure(condition: bool, message: &'static str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn ensure_cli_success(output: &std::process::Output) -> TestResult {
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "CLI failed with status {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

fn parse_cli_result(output: &std::process::Output) -> Result<Value, TestError> {
    ensure_cli_success(output)?;
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    let result = envelope
        .get("result")
        .cloned()
        .ok_or_else(|| "CLI result envelope omitted result".to_owned())?;
    Ok(result.get("record").cloned().unwrap_or(result))
}

fn human_actor(value: &str) -> Result<Identity, IdentityValidationError> {
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
