use collaboration_client::ControlClient;
use collaboration_protocol::{
    ThreadSubscribeRequest, ThreadSubscriptionPresence, ThreadSubscriptionState,
    ThreadSubscriptionsRequest, ThreadUnsubscribeRequest,
};
use collaboration_service::{
    BoardAvailability, MachineIdentity, SessionDeliveryRouter, SessionMessageDelivery,
    SubscriptionDeliveryService, SubscriptionDeliveryServiceProps, SystemSubscriptionClock,
    TargetPresenceProbe,
};
use collaboration_service::{ServiceIdentity, serve_control_connection};
use message_board::{
    BoardCreateRequest, BoardId, Description, EndpointId, Identity, MessageId, MessageReferences,
    MessageText, ParticipantRole, ProjectCreateRequest, ProjectId, ResourceName, ServiceId,
    SessionEndpointRef, SessionId, SessionRef, SubscriptionMode, SubscriptionPolicyPatch,
    SubscriptionScope, ThreadCreateRequest, ThreadJoinRequest, TopicCreateRequest, TopicId,
    WhenIdle,
};
use message_board_storage::BoardStore;
use std::sync::Arc;
use tokio::sync::Mutex;

fn ensure_subscription_condition(
    condition: bool,
    message: &'static str,
) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message).into())
    }
}

#[tokio::test]
async fn subscription_control_methods_roundtrip_through_control_and_sqlite()
-> Result<(), Box<dyn std::error::Error>> {
    const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
    let directory = tempfile::tempdir()?;
    let board_path = directory.path().join("board.sqlite");
    let automation_path = directory.path().join("automation.sqlite");
    let store = Arc::new(Mutex::new(BoardStore::open(&board_path).await?));
    let push_store = Arc::new(Mutex::new(
        automation_storage::AutomationStore::open(&automation_path).await?,
    ));
    let router = Arc::new(SessionDeliveryRouter::new(Vec::new()));
    let delivery: Arc<dyn SessionMessageDelivery> = router.clone();
    let presence: Arc<dyn TargetPresenceProbe> = router.clone();
    let machine_identity = MachineIdentity::new(
        collaboration_protocol::UuidIdentity::try_from(SERVICE_ID.to_owned())?,
        None,
    )?;
    let clock: Arc<dyn collaboration_service::SubscriptionClock> =
        Arc::new(SystemSubscriptionClock);
    let subscription_delivery =
        SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
            board_availability: BoardAvailability::Available(Arc::clone(&store)),
            push_store: Arc::clone(&push_store),
            delivery,
            presence: Arc::clone(&presence),
            machine_identity,
            clock: Arc::clone(&clock),
        });
    subscription_delivery.start().await?;
    let identity = ServiceIdentity::new(
        SERVICE_ID,
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .map_err(std::io::Error::other)?
    .with_board_store(Arc::clone(&store))
    .with_subscription_delivery_service(subscription_delivery.clone(), presence);
    let (socket, server) = tokio::net::UnixStream::pair()?;
    let server_task = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(socket, "subscription-control-test", "1").await?;

    let reader = Identity::Human {
        human_id: "human-reader".to_owned().try_into()?,
    };
    let project_id = ProjectId::generate();
    client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("Project".to_owned())?,
            description: Description::try_from(String::new())?,
            actor: reader.clone(),
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
            actor: reader.clone(),
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
            actor: reader.clone(),
            acting_for: None,
        })
        .await?;

    let subscribed = client
        .board_thread_subscribe(ThreadSubscribeRequest {
            actor: reader.clone(),
            scope: SubscriptionScope::topic(topic_id.clone()),
            policy: SubscriptionPolicyPatch::default(),
        })
        .await
        .map_err(|error| std::io::Error::other(format!("board/threadSubscribe failed: {error}")))?;
    ensure_subscription_condition(
        subscribed.scope == SubscriptionScope::topic(topic_id.clone()),
        "subscribe result lost its Topic scope",
    )?;
    ensure_subscription_condition(
        subscribed.state == ThreadSubscriptionState::Active,
        "subscribe result is not active",
    )?;
    ensure_subscription_condition(
        subscribed.policy.mode() == SubscriptionMode::Off,
        "human Topic subscription did not default to mode off",
    )?;
    ensure_subscription_condition(
        subscribed.presence
            == ThreadSubscriptionPresence::Unreachable {
                reason: "no session target".to_owned(),
            },
        "human subscription did not report its missing session target",
    )?;

    let listed = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: reader.clone(),
        })
        .await
        .map_err(|error| {
            std::io::Error::other(format!("board/threadSubscriptions failed: {error}"))
        })?;
    ensure_subscription_condition(
        listed.subscriptions == vec![subscribed],
        "list did not return the persisted subscription view",
    )?;

    let created_thread = client
        .board_thread_create(ThreadCreateRequest {
            message_id: MessageId::generate(),
            topic_id: topic_id.clone(),
            actor: reader.clone(),
            acting_for: None,
            text: MessageText::try_from("Join policy".to_owned())?,
            references: MessageReferences::try_from(Vec::new())?,
            role: None,
            watch: false,
        })
        .await?;
    let first_root_watch = store
        .lock()
        .await
        .show_thread(message_board::ThreadShowRequest {
            root_message_id: created_thread.message.message_id.clone(),
            reader: Some(reader.clone()),
        })
        .await?
        .watch_status
        .ok_or("empty-topic subscriber did not receive a first-root watch status")?;
    ensure_subscription_condition(
        first_root_watch.watching
            && first_root_watch
                .starts_after_activity_sequence
                .map(|sequence| sequence.get())
                == Some(0),
        "empty-topic subscription did not cover its first later root from boundary zero",
    )?;

    let cancelled = client
        .board_thread_unsubscribe(ThreadUnsubscribeRequest {
            actor: reader.clone(),
            scope: SubscriptionScope::topic(topic_id.clone()),
        })
        .await
        .map_err(|error| {
            std::io::Error::other(format!("board/threadUnsubscribe failed: {error}"))
        })?;
    ensure_subscription_condition(
        cancelled.state == ThreadSubscriptionState::Ended
            && cancelled.end_reason == Some(message_board::EndReason::Cancelled),
        "unsubscribe did not return the cancelled terminal state",
    )?;
    let remaining_human_subscriptions = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: reader.clone(),
        })
        .await?;
    ensure_subscription_condition(
        remaining_human_subscriptions.subscriptions.is_empty(),
        "unsubscribe left an active human subscription in the list",
    )?;

    let session_reader = Identity::Session {
        session: SessionRef {
            endpoint: SessionEndpointRef {
                service_id: ServiceId::try_from(SERVICE_ID.to_owned())?,
                endpoint_id: EndpointId::try_from("codex-local".to_owned())?,
            },
            session_id: SessionId::try_from("00000000-0000-7000-8000-000000000003".to_owned())?,
        },
    };
    client
        .board_thread_join(ThreadJoinRequest {
            root_message_id: created_thread.message.message_id,
            actor: session_reader.clone(),
            role: ParticipantRole::Participant,
            watch: true,
            mode: Some(SubscriptionMode::Poll),
            when_idle: Some(WhenIdle::Hold),
            replace: None,
            note: None,
        })
        .await
        .map_err(|error| std::io::Error::other(format!("board/threadJoin failed: {error}")))?;
    let joined_subscription = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: session_reader.clone(),
        })
        .await?
        .subscriptions
        .into_iter()
        .next()
        .ok_or("joining with watch did not create a subscription")?;
    ensure_subscription_condition(
        joined_subscription.policy.mode() == SubscriptionMode::Poll
            && joined_subscription.policy.when_idle() == WhenIdle::Hold,
        "thread join did not apply its optional mode and idle policy",
    )?;
    client
        .board_thread_unsubscribe(ThreadUnsubscribeRequest {
            actor: session_reader.clone(),
            scope: joined_subscription.scope,
        })
        .await?;
    let remaining_session_subscriptions = client
        .board_thread_subscriptions(ThreadSubscriptionsRequest {
            actor: session_reader,
        })
        .await?;
    ensure_subscription_condition(
        remaining_session_subscriptions.subscriptions.is_empty(),
        "session unsubscribe left an active subscription in the list",
    )?;

    drop(client);
    server_task.await??;
    subscription_delivery.shutdown().await;
    drop(subscription_delivery);
    let store = Arc::try_unwrap(store)
        .map_err(|_| "Control service retained the board store after connection shutdown")?;
    store.into_inner().close().await?;
    let push_store = Arc::try_unwrap(push_store)
        .map_err(|_| "Subscription service retained automation storage after shutdown")?;
    push_store.into_inner().close().await?;
    Ok(())
}
