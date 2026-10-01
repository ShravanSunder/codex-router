use crate::mcp_server::CollaborationMcpServer;
use collaboration_client::ControlClient;
use collaboration_protocol::{ServiceManifest, UuidIdentity};
use collaboration_service::{
    BoardAvailability, LocalControlService, MachineIdentity, ManifestPublication, ServiceIdentity,
    SessionDeliveryRouter, SessionMessageDelivery, SubscriptionDeliveryService,
    SubscriptionDeliveryServiceProps, SystemSubscriptionClock, TargetPresenceProbe,
};
use message_board::{
    BoardCreateRequest, BoardId, Description, Identity, MessageId, MessageText,
    ProjectCreateRequest, ProjectId, ResourceName, ThreadCreateRequest, TopicCreateRequest,
    TopicId,
};
use message_board_storage::BoardStore;
use rmcp::{ServerHandler as _, ServiceExt as _, handler::client::ClientHandler};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000011";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000012";
const CONTROL_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[derive(Clone)]
struct TestMcpClient;

impl ClientHandler for TestMcpClient {}

async fn call_registered_tool(
    server: &CollaborationMcpServer,
    request_context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    name: &str,
    arguments: Value,
) -> rmcp::model::CallToolResult {
    let arguments = arguments
        .as_object()
        .expect("MCP tool arguments are an object")
        .clone();
    let response = server
        .call_tool(
            rmcp::model::CallToolRequestParams::new(name.to_owned()).with_arguments(arguments),
            request_context,
        )
        .await
        .expect("MCP tool call");
    let rmcp::model::CallToolResponse::Complete(result) = response else {
        panic!("board tool returns one complete result")
    };
    result
}

#[tokio::test]
async fn subscription_tools_roundtrip_through_mcp_control_and_sqlite() {
    use std::os::unix::fs::PermissionsExt;

    let temporary = tempfile::tempdir_in("/tmp").expect("private service directory");
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private service permissions");

    let board_store = Arc::new(Mutex::new(
        BoardStore::open(&temporary.path().join("board.sqlite"))
            .await
            .expect("real board SQLite store"),
    ));
    let push_store = Arc::new(Mutex::new(
        automation_storage::AutomationStore::open(&temporary.path().join("automation.sqlite"))
            .await
            .expect("real push SQLite store"),
    ));
    let delivery_router = Arc::new(SessionDeliveryRouter::new(Vec::new()));
    let delivery: Arc<dyn SessionMessageDelivery> = delivery_router.clone();
    let presence: Arc<dyn TargetPresenceProbe> = delivery_router;
    let service_id = UuidIdentity::try_from(SERVICE_ID.to_owned()).expect("service id");
    let subscription_service = SubscriptionDeliveryService::new(SubscriptionDeliveryServiceProps {
        board_availability: BoardAvailability::Available(Arc::clone(&board_store)),
        push_store,
        delivery,
        presence: Arc::clone(&presence),
        machine_identity: MachineIdentity::new(service_id, Some("mcp-subscription-test"))
            .expect("machine identity"),
        clock: Arc::new(SystemSubscriptionClock),
    });
    subscription_service
        .start()
        .await
        .expect("subscription service start");

    let service_identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, CONTROL_DIGEST)
        .expect("Control service identity")
        .with_board_store(Arc::clone(&board_store))
        .with_subscription_delivery_service(subscription_service.clone(), presence);
    let control =
        LocalControlService::bind(&temporary.path().join("control.sock"), service_identity)
            .expect("Control listener");
    let manifest: ServiceManifest = serde_json::from_value(json!({
        "version":2,
        "serviceId":SERVICE_ID,
        "serviceEpoch":SERVICE_EPOCH,
        "machineLabel":"mcp-subscription-test",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":CONTROL_DIGEST,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let _publication = ManifestPublication::publish(temporary.path(), &manifest)
        .expect("publish real Control manifest");
    let control_shutdown = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_shutdown.clone()));

    let actor: Identity = serde_json::from_value(json!({
        "kind":"session",
        "session":{
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"codex-local"},
            "sessionId":"mcp-subscription-reader"
        }
    }))
    .expect("session actor");
    let setup_actor: Identity = serde_json::from_value(json!({
        "kind":"human",
        "humanId":"mcp-subscription-owner"
    }))
    .expect("human setup actor");
    let mut setup_client = ControlClient::connect(temporary.path(), "subscription-test", "1")
        .await
        .expect("setup Control client");
    let project_id = ProjectId::generate();
    setup_client
        .board_project_create(ProjectCreateRequest {
            project_id: project_id.clone(),
            name: ResourceName::try_from("MCP subscriptions".to_owned()).expect("project name"),
            description: Description::try_from(String::new()).expect("project description"),
            actor: setup_actor.clone(),
            acting_for: None,
        })
        .await
        .expect("create project in SQLite");
    let board_id = BoardId::generate();
    setup_client
        .board_create(BoardCreateRequest {
            board_id: board_id.clone(),
            project_id,
            name: ResourceName::try_from("MCP board".to_owned()).expect("board name"),
            description: Description::try_from(String::new()).expect("board description"),
            actor: setup_actor.clone(),
            acting_for: None,
        })
        .await
        .expect("create board in SQLite");
    let topic_id = TopicId::generate();
    setup_client
        .board_topic_create(TopicCreateRequest {
            topic_id: topic_id.clone(),
            board_id,
            name: ResourceName::try_from("MCP topic".to_owned()).expect("topic name"),
            description: Description::try_from(String::new()).expect("topic description"),
            actor: setup_actor.clone(),
            acting_for: None,
        })
        .await
        .expect("create topic in SQLite");
    let thread = setup_client
        .board_thread_create(ThreadCreateRequest {
            message_id: MessageId::generate(),
            topic_id,
            actor: setup_actor,
            acting_for: None,
            text: MessageText::try_from("MCP subscription entry path".to_owned())
                .expect("thread title"),
            references: Vec::<message_board::ReferenceTarget>::new()
                .try_into()
                .expect("thread references"),
            role: None,
            watch: false,
        })
        .await
        .expect("create thread in SQLite");
    let root_message_id = thread.message.message_id;
    setup_client
        .close()
        .await
        .expect("close setup Control client");

    let mcp_server = CollaborationMcpServer::new(temporary.path().to_owned());
    let handler = mcp_server.clone();
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (running_server, running_client) = tokio::join!(
        mcp_server.serve(server_transport),
        TestMcpClient.serve(client_transport),
    );
    let running_server = running_server.expect("MCP server transport");
    let running_client = running_client.expect("MCP test client transport");
    let context_for = |request_id: i64| {
        rmcp::service::RequestContext::new(
            rmcp::model::NumberOrString::Number(request_id),
            running_server.peer().clone(),
        )
    };

    let joined = call_registered_tool(
        &handler,
        context_for(1),
        "board_thread_join",
        json!({
            "rootMessageId":root_message_id,
            "actor":actor,
            "role":"participant",
            "watch":true,
            "mode":"poll"
        }),
    )
    .await;
    assert_ne!(joined.is_error, Some(true));
    assert!(
        joined.structured_content.expect("join structured result")["watchStatus"]["watching"]
            .as_bool()
            .unwrap_or(false)
    );

    let listed = call_registered_tool(
        &handler,
        context_for(2),
        "board_thread_subscriptions",
        json!({"actor":actor}),
    )
    .await;
    assert_ne!(listed.is_error, Some(true));
    let listed = listed
        .structured_content
        .expect("subscriptions structured result");
    assert_eq!(listed["subscriptions"].as_array().map(Vec::len), Some(1));
    assert_eq!(listed["subscriptions"][0]["policy"]["mode"], "poll");

    let updated = call_registered_tool(
        &handler,
        context_for(3),
        "board_thread_subscribe",
        json!({
            "actor":actor,
            "scope":{"kind":"thread","rootMessageId":root_message_id},
            "policy":{"mode":"off","quietSeconds":0,"capSeconds":60,"lifetimeSeconds":600}
        }),
    )
    .await;
    assert_ne!(updated.is_error, Some(true));
    assert_eq!(
        updated
            .structured_content
            .expect("subscribe structured result")["policy"]["mode"],
        "off"
    );

    let unsubscribed = call_registered_tool(
        &handler,
        context_for(4),
        "board_thread_unsubscribe",
        json!({
            "actor":actor,
            "scope":{"kind":"thread","rootMessageId":root_message_id}
        }),
    )
    .await;
    assert_ne!(unsubscribed.is_error, Some(true));
    let unsubscribed = unsubscribed
        .structured_content
        .expect("unsubscribe structured result");
    assert_eq!(unsubscribed["state"], "ended");
    assert_eq!(unsubscribed["endReason"]["kind"], "cancelled");

    let remaining = call_registered_tool(
        &handler,
        context_for(5),
        "board_thread_subscriptions",
        json!({"actor":actor}),
    )
    .await;
    assert_ne!(remaining.is_error, Some(true));
    assert_eq!(
        remaining
            .structured_content
            .expect("remaining subscriptions")["subscriptions"],
        json!([])
    );

    let thread = call_registered_tool(
        &handler,
        context_for(6),
        "board_thread_show",
        json!({"rootMessageId":root_message_id,"reader":actor}),
    )
    .await;
    assert_ne!(thread.is_error, Some(true));
    assert!(
        thread.structured_content.expect("thread show result")["watchStatus"]["watching"]
            .as_bool()
            .unwrap_or(false),
        "unsubscribe leaves the Watch active"
    );

    running_client.cancel().await.expect("stop MCP test client");
    running_server.cancel().await.expect("stop MCP test server");
    subscription_service.shutdown().await;
    control_shutdown.cancel();
    control_task
        .await
        .expect("Control service task")
        .expect("Control service shutdown");
}
