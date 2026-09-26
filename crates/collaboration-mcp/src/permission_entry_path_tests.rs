use crate::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
use codex_acp_adapter::{
    AcpSchemaCatalog, ApprovalBroker, ApprovalRoute, BrokeredApprovalOutcome,
    BrokeredApprovalRequest, PendingPermission,
};
use collaboration_protocol::{
    ApprovalDecision, CodexGeneration, EndpointDescription, EndpointRef, RouterAccess, SessionRef,
};
use collaboration_service::{
    EndpointDirectory, LocalControlService, ManifestPublication, NativeControlBackend,
    NativeGenerationGate, ServiceApprovalBroker, ServiceIdentity,
};
use futures_util::{SinkExt, StreamExt};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000011";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000012";

struct ApprovalFixture {
    directory: tempfile::TempDir,
    broker: Arc<ServiceApprovalBroker>,
    generation: CodexGeneration,
    requester: SessionRef,
    approver: SessionRef,
    control_stop: CancellationToken,
    control_task: tokio::task::JoinHandle<std::io::Result<()>>,
    fault_task: Option<tokio::task::JoinHandle<()>>,
    forwarded_decisions: Option<Arc<AtomicUsize>>,
    backend_task: tokio::task::JoinHandle<()>,
    delivery_rx: tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<usize>>,
    publication: ManifestPublication,
}

impl ApprovalFixture {
    async fn start(delivery_count: usize) -> Self {
        Self::start_with_dropped_decision_reply(delivery_count, false).await
    }

    async fn start_with_dropped_decision_reply(
        delivery_count: usize,
        drop_decision_reply: bool,
    ) -> Self {
        let directory = tempfile::tempdir_in("/tmp").expect("private service directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private permissions");
        }
        let service_id: collaboration_protocol::UuidIdentity =
            SERVICE_ID.to_owned().try_into().expect("service id");
        let endpoint = EndpointRef {
            service_id: service_id.clone(),
            endpoint_id: "codex-local".to_owned().try_into().expect("endpoint id"),
        };
        let requester: SessionRef = serde_json::from_value(json!({
            "endpoint": endpoint, "sessionId": "permission-requester"
        }))
        .expect("requester");
        let approver: SessionRef = serde_json::from_value(json!({
            "endpoint": requester.endpoint, "sessionId": "permission-approver"
        }))
        .expect("approver");
        let generation: CodexGeneration = serde_json::from_value(json!({
            "serviceEpoch": SERVICE_EPOCH, "generation": 7
        }))
        .expect("generation");

        let (schemas, digest) = native_message_schemas();
        let backend_path = directory.path().join("native.sock");
        let backend_listener = tokio::net::UnixListener::bind(&backend_path).expect("native bind");
        let (delivery_tx, delivery_rx) = tokio::sync::mpsc::unbounded_channel();
        let backend_task = tokio::spawn(serve_approval_deliveries(
            backend_listener,
            delivery_count,
            approver.clone(),
            delivery_tx,
        ));
        let gate = NativeGenerationGate::default();
        gate.activate(
            generation.clone(),
            backend_path.clone(),
            Some(Arc::clone(&schemas)),
        )
        .expect("activate generation");
        let description: EndpointDescription = serde_json::from_value(json!({
            "endpoint": requester.endpoint,
            "label": "Permission integration fixture",
            "availability": {"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels": [{
                "kind":"nativeCodex", "transport":"unixWebSocket", "path":"native.sock",
                "schemaDigest":digest, "generation":generation
            }]
        }))
        .expect("endpoint description");
        let endpoints = EndpointDirectory::new(service_id.clone());
        endpoints
            .publish(description.clone())
            .expect("publish endpoint");
        let native_backend = NativeControlBackend {
            endpoint: requester.endpoint.clone(),
            gate: gate.clone(),
            codex_home: directory.path().to_owned(),
        };
        let broker = ServiceApprovalBroker::load(
            service_id.clone(),
            native_backend.clone(),
            directory.path().join("approval-routes.json"),
        )
        .await
        .expect("approval broker");
        let route: Arc<dyn collaboration_service::SessionDeliveryRoute> =
            Arc::new(collaboration_service::CodexAppServerDeliveryRoute::new(
                service_id.clone(),
                endpoints,
                native_backend.clone(),
                Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
            ));
        broker
            .install_session_delivery(Arc::new(collaboration_service::SessionDeliveryRouter::new(
                vec![route],
            )))
            .expect("delivery injection");
        broker
            .register_route(ApprovalRoute {
                thread_id: String::from(requester.session_id.clone()),
                created_by: requester.clone(),
                approver: approver.clone(),
                access: RouterAccess::WorkspaceWrite,
                scratch_path: directory.path().display().to_string(),
                root_message_id: None,
            })
            .await
            .expect("approval route");
        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH, &digest)
            .expect("service identity")
            .with_endpoints(vec![description])
            .expect("service endpoint")
            .with_native_backend(native_backend)
            .expect("native backend")
            .with_approval_broker(Arc::clone(&broker));
        let control_path = directory.path().join(if drop_decision_reply {
            "broker-control.sock"
        } else {
            "control.sock"
        });
        let control = LocalControlService::bind(&control_path, identity).expect("control bind");
        let forwarded_decisions = drop_decision_reply.then(|| Arc::new(AtomicUsize::new(0)));
        let fault_task = forwarded_decisions.as_ref().map(|forwarded_decisions| {
            let listener = tokio::net::UnixListener::bind(directory.path().join("control.sock"))
                .expect("fault proxy bind");
            let target = control_path.clone();
            let forwarded_decisions = Arc::clone(forwarded_decisions);
            tokio::spawn(async move {
                drop_approval_decision_reply(listener, target, forwarded_decisions).await;
            })
        });
        let manifest = serde_json::from_value(json!({
            "version":2, "serviceId":SERVICE_ID, "serviceEpoch":SERVICE_EPOCH,
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("manifest");
        let publication = ManifestPublication::publish(directory.path(), &manifest)
            .expect("manifest publication");
        let control_stop = CancellationToken::new();
        let control_task = tokio::spawn(control.run(control_stop.clone()));
        Self {
            directory,
            broker,
            generation,
            requester,
            approver,
            control_stop,
            control_task,
            fault_task,
            forwarded_decisions,
            backend_task,
            delivery_rx: tokio::sync::Mutex::new(delivery_rx),
            publication,
        }
    }

    fn service_directory(&self) -> &Path {
        self.directory.path()
    }

    async fn begin_permission_request(
        &self,
        suffix: &str,
    ) -> tokio::task::JoinHandle<
        Result<BrokeredApprovalOutcome, codex_acp_adapter::ApprovalBrokerError>,
    > {
        self.begin_permission_request_for_generation(suffix, self.generation.clone())
            .await
    }

    async fn begin_permission_request_for_generation(
        &self,
        suffix: &str,
        generation: CodexGeneration,
    ) -> tokio::task::JoinHandle<
        Result<BrokeredApprovalOutcome, codex_acp_adapter::ApprovalBrokerError>,
    > {
        let mut schema = AcpSchemaCatalog::load().expect("ACP schemas");
        let native = json!({
            "id":format!("native-{suffix}"),
            "method":"item/permissions/requestApproval",
            "params":{
                "threadId":String::from(self.requester.session_id.clone()),
                "turnId":format!("turn-{suffix}"),
                "itemId":format!("permission-{suffix}"),
                "startedAtMs":1,
                "cwd":self.service_directory(),
                "reason":"write deterministic permission proof",
                "permissions":{
                    "network":{"enabled":true},
                    "fileSystem":{"read":[],"write":[self.service_directory()]}
                }
            }
        });
        let permission = PendingPermission::translate(
            &mut schema,
            generation.clone(),
            format!("acp-{suffix}"),
            &native,
        )
        .expect("translate native permission callback");
        let request = permission.request().clone();
        let broker = Arc::clone(&self.broker);
        let thread_id = String::from(self.requester.session_id.clone());
        tokio::spawn(async move {
            broker
                .request(BrokeredApprovalRequest {
                    thread_id,
                    generation,
                    request,
                })
                .await
        })
    }

    async fn pending_record(&self) -> collaboration_protocol::ApprovalRequestRecord {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(record) = self.broker.list(true).await.approvals.into_iter().next() {
                    return record;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("pending approval")
    }

    async fn await_delivery(&self, expected: usize) {
        let delivered =
            tokio::time::timeout(Duration::from_secs(5), self.delivery_rx.lock().await.recv())
                .await
                .expect("approval delivery deadline")
                .expect("approval delivery signal");
        assert_eq!(delivered, expected);
    }

    fn forwarded_decision_count(&self) -> usize {
        self.forwarded_decisions
            .as_ref()
            .map_or(0, |count| count.load(Ordering::SeqCst))
    }

    async fn shutdown(self) {
        self.control_stop.cancel();
        self.control_task
            .await
            .expect("control join")
            .expect("control shutdown");
        if let Some(fault_task) = self.fault_task {
            fault_task.await.expect("fault proxy join");
        }
        self.backend_task.await.expect("backend join");
        drop(self.publication);
    }
}

fn native_message_schemas() -> (Arc<codex_native_integration::NativePayloadSchemas>, String) {
    let mut definitions = BTreeMap::new();
    for operation in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadNameSet",
    ] {
        definitions.insert(format!("{operation}Params"), json!({"type":"object"}));
        definitions.insert(format!("{operation}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}})).expect("schema JSON"),
    )]))
    .expect("native schema bundle");
    let schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .expect("native payload schemas"),
    );
    let digest = schemas.schema_digest().to_owned();
    (schemas, digest)
}

async fn serve_approval_deliveries(
    listener: tokio::net::UnixListener,
    delivery_count: usize,
    approver: SessionRef,
    delivery_tx: tokio::sync::mpsc::UnboundedSender<usize>,
) {
    for index in 0..delivery_count {
        let (stream, _) = listener.accept().await.expect("native accept");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("native websocket");
        for expected_method in ["initialize", "initialized", "thread/read", "turn/start"] {
            let frame = socket
                .next()
                .await
                .expect("native frame")
                .expect("native receive");
            let request: Value =
                serde_json::from_str(frame.to_text().expect("native text")).expect("native JSON");
            assert_eq!(request["method"], expected_method);
            if expected_method == "initialized" {
                continue;
            }
            let result = match expected_method {
                "initialize" => json!({"userAgent":"permission-fixture"}),
                "thread/read" => json!({"thread":{
                    "id":String::from(approver.session_id.clone()), "cwd":"/tmp",
                    "status":{"type":"idle"}, "createdAt":1, "updatedAt":1,
                    "sandbox":{"type":"workspaceWrite"}, "approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review"
                }}),
                "turn/start" => {
                    let text = request["params"]["input"][0]["text"]
                        .as_str()
                        .expect("approval delivery text");
                    let record: collaboration_protocol::ApprovalRequestRecord =
                        serde_json::from_str(
                            text.rsplit_once("\n\n").map_or(text, |(_, value)| value),
                        )
                        .expect("delivered approval record");
                    assert_eq!(record.approver, approver);
                    assert_eq!(
                        String::from(record.requester.session_id.clone()),
                        "permission-requester"
                    );
                    assert_eq!(
                        serde_json::to_value(&record.generation).expect("generation JSON")["generation"],
                        if delivery_count == 2 && index == 1 {
                            6
                        } else {
                            7
                        }
                    );
                    assert!(
                        record.operation["params"]["toolCall"]["content"][0]["content"]["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("Requested permissions:")
                                && text.contains("\"enabled\": true"))
                    );
                    json!({"turn":{"id":format!("approval-delivery-{index}")}})
                }
                _ => json!({}),
            };
            socket
                .send(Message::Text(
                    json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("native response");
        }
        delivery_tx.send(index).expect("approval delivery signal");
    }
}

async fn drop_approval_decision_reply(
    listener: tokio::net::UnixListener,
    target: PathBuf,
    forwarded_decisions: Arc<AtomicUsize>,
) {
    let (client, _) = listener.accept().await.expect("fault proxy accept");
    let broker = tokio::net::UnixStream::connect(target)
        .await
        .expect("fault proxy broker connect");
    let (client_read, mut client_write) = client.into_split();
    let (broker_read, mut broker_write) = broker.into_split();
    let mut client_lines = BufReader::new(client_read).lines();
    let mut broker_lines = BufReader::new(broker_read).lines();

    while let Some(request) = client_lines
        .next_line()
        .await
        .expect("fault proxy client frame")
    {
        let method =
            serde_json::from_str::<Value>(&request).expect("fault proxy client JSON")["method"]
                .as_str()
                .map(str::to_owned);
        broker_write
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("fault proxy forward request");
        let response = broker_lines
            .next_line()
            .await
            .expect("fault proxy broker frame")
            .expect("fault proxy broker response");
        if method.as_deref() == Some("approval/decide") {
            forwarded_decisions.fetch_add(1, Ordering::SeqCst);
            return;
        }
        client_write
            .write_all(format!("{response}\n").as_bytes())
            .await
            .expect("fault proxy forward response");
    }
}

fn cli_actor(actor: &SessionRef) -> String {
    serde_json::to_string(actor).expect("actor JSON")
}

async fn run_cli_decision(directory: &Path, request_id: &str, actor: &SessionRef) -> i32 {
    let arguments = vec![
        OsString::from("agent-collaboration approval"),
        OsString::from("decide"),
        OsString::from("--request-id"),
        OsString::from(request_id),
        OsString::from("--allow"),
        OsString::from("--actor"),
        OsString::from(cli_actor(actor)),
        OsString::from("--service-directory"),
        directory.as_os_str().to_owned(),
        OsString::from("--json"),
    ];
    tokio::task::spawn_blocking(move || agent_collaboration::run_approval_command(arguments))
        .await
        .expect("CLI decision join")
}

#[tokio::test]
async fn cli_decision_response_loss_after_real_broker_effect_is_unknown_without_replay() {
    let fixture = ApprovalFixture::start_with_dropped_decision_reply(1, true).await;
    let request = fixture.begin_permission_request("cli-response-loss").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;

    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &fixture.approver,
        )
        .await,
        5,
        "the actual CLI entry projects post-dispatch response loss as unknown",
    );
    assert_eq!(
        fixture.forwarded_decision_count(),
        1,
        "the SDK did not replay"
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        },
        "the real broker applied the decision before its Control receipt was lost",
    );
    let approvals = fixture.broker.list(false).await.approvals;
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].request_id, pending.request_id);
    assert_eq!(approvals[0].decision, Some(ApprovalDecision::Allow));
    fixture.shutdown().await;
}

#[tokio::test]
async fn cli_command_entry_authorizes_real_broker_permission_by_actor_target_and_generation() {
    let fixture = ApprovalFixture::start(2).await;
    let request = fixture.begin_permission_request("cli").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;
    assert_eq!(pending.requester, fixture.requester);
    assert_eq!(pending.approver, fixture.approver);
    assert_eq!(pending.generation, fixture.generation);
    assert!(
        pending.operation["params"]["toolCall"]["content"][0]["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Requested permissions:"))
    );

    let wrong_actor: SessionRef = serde_json::from_value(json!({
        "endpoint":fixture.approver.endpoint,"sessionId":"wrong-approver"
    }))
    .expect("wrong actor");
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &wrong_actor
        )
        .await,
        4
    );
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &pending.request_id,
            &fixture.approver
        )
        .await,
        0
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        }
    );

    let mut stale_generation = fixture.generation.clone();
    stale_generation.generation = 6_u64.try_into().expect("stale generation");
    let stale_request = fixture
        .begin_permission_request_for_generation("cli-stale", stale_generation)
        .await;
    let stale = fixture.pending_record().await;
    fixture.await_delivery(1).await;
    assert_ne!(stale.generation, fixture.generation);
    assert_eq!(
        run_cli_decision(
            fixture.service_directory(),
            &stale.request_id,
            &fixture.approver
        )
        .await,
        4
    );
    assert_eq!(
        stale_request
            .await
            .expect("stale request join")
            .expect("stale broker request"),
        BrokeredApprovalOutcome::Cancelled
    );
    fixture.shutdown().await;
}

async fn mcp_call(
    client: &reqwest::Client,
    url: &str,
    session_id: &reqwest::header::HeaderValue,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let response = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id)
        .json(&json!({
            "jsonrpc":"2.0", "id":id, "method":"tools/call",
            "params":{"name":name,"arguments":arguments}
        }))
        .send()
        .await
        .expect("MCP call");
    let status = response.status();
    let body = response.text().await.expect("MCP response body");
    let payload = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(&body);
    serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("MCP response JSON: {error}; status={status}; body={body:?}")
    })
}

#[tokio::test]
async fn streamable_http_entry_authorizes_real_broker_permission_by_actor_target_and_generation() {
    let fixture = ApprovalFixture::start(1).await;
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: fixture.service_directory().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"permission-integration","version":"1"}
            }
        }))
        .send()
        .await
        .expect("initialize");
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session id");
    let _initialize_body = initialize.text().await.expect("initialize body");
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .expect("initialized notification");
    assert!(initialized.status().is_success());

    let request = fixture.begin_permission_request("mcp").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;
    let list = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        2,
        "approval_list",
        json!({"pending":true}),
    )
    .await;
    let listed = &list["result"]["structuredContent"]["approvals"][0];
    assert_eq!(listed["requestId"], pending.request_id);
    assert_eq!(listed["requester"], serde_json::json!(fixture.requester));
    assert_eq!(listed["approver"], serde_json::json!(fixture.approver));
    assert_eq!(listed["generation"], serde_json::json!(fixture.generation));
    assert!(
        listed["operation"]["params"]["toolCall"]["content"][0]["content"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Requested permissions:"))
    );

    let wrong_actor: SessionRef = serde_json::from_value(json!({
        "endpoint":fixture.approver.endpoint,"sessionId":"wrong-approver"
    }))
    .expect("wrong actor");
    let rejected = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        3,
        "approval_decide",
        json!({
            "requestId":pending.request_id,"decision":"allow",
            "actor":wrong_actor
        }),
    )
    .await;
    assert_eq!(rejected["result"]["isError"], true);
    assert_eq!(
        rejected["result"]["structuredContent"]["data"]["kind"],
        "wrongActor"
    );
    let accepted = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        4,
        "approval_decide",
        json!({
            "requestId":pending.request_id,"decision":"allowForSession",
            "actor":fixture.approver
        }),
    )
    .await;
    assert_eq!(accepted["result"]["isError"], false);
    assert_eq!(
        accepted["result"]["structuredContent"]["decision"],
        serde_json::json!(ApprovalDecision::AllowForSession)
    );
    assert_eq!(
        accepted["result"]["structuredContent"]["scope"],
        "nativeSession"
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept-session".to_owned()
        }
    );
    listener.shutdown().await.expect("MCP shutdown");
    fixture.shutdown().await;
}

#[tokio::test]
async fn initialized_http_decision_response_loss_after_real_broker_effect_is_unknown_without_replay()
 {
    let fixture = ApprovalFixture::start_with_dropped_decision_reply(1, true).await;
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: fixture.service_directory().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"approval-response-loss","version":"1"}
            }
        }))
        .send()
        .await
        .expect("initialize");
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session id");
    let _body = initialize.text().await.expect("initialize body");
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .expect("initialized notification");
    assert!(initialized.status().is_success());

    let request = fixture.begin_permission_request("mcp-response-loss").await;
    let pending = fixture.pending_record().await;
    fixture.await_delivery(0).await;
    let response = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        2,
        "approval_decide",
        json!({
            "requestId":pending.request_id,"decision":"allow","actor":fixture.approver
        }),
    )
    .await;
    let failure = &response["result"]["structuredContent"];
    assert_eq!(response["result"]["isError"], true);
    assert_eq!(failure["effect"], "unknown");
    assert_eq!(failure["stage"], "approval-decision");
    assert_eq!(
        fixture.forwarded_decision_count(),
        1,
        "the SDK did not replay"
    );
    assert_eq!(
        request
            .await
            .expect("request join")
            .expect("broker request"),
        BrokeredApprovalOutcome::Selected {
            option_id: "native-accept".to_owned()
        },
        "the real broker applied the decision before its Control receipt was lost",
    );
    let approvals = fixture.broker.list(false).await.approvals;
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].request_id, pending.request_id);
    assert_eq!(approvals[0].decision, Some(ApprovalDecision::Allow));
    listener.shutdown().await.expect("MCP shutdown");
    fixture.shutdown().await;
}

#[tokio::test]
async fn initialized_http_create_reports_manifest_preflight_without_creation_uncertainty() {
    // The listener is real and initialized, but its service directory deliberately
    // has no manifest. This reaches the MCP adapter's actual create entry point
    // while proving that discovery failure occurred before ACP setup/dispatch.
    let directory = tempfile::tempdir_in("/tmp").expect("private service directory");
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: directory.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"fault-entry-proof","version":"1"}
            }
        }))
        .send()
        .await
        .expect("initialize");
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session id");
    let _body = initialize.text().await.expect("initialize body");
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .expect("initialized notification");
    assert!(initialized.status().is_success());

    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"codex-local"});
    let local_identity = json!({"endpoint":endpoint,"sessionId":"mcp-caller"});
    let foreign_identity = json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000099","endpointId":"codex-local"},
        "sessionId":"foreign-caller"
    });
    for (id, created_by, approver) in [
        (2, Value::Null, local_identity.clone()),
        (3, foreign_identity, local_identity.clone()),
    ] {
        let missing_creator = created_by.is_null();
        let response = mcp_call(
            &client,
            &listener.local_url(),
            &session_id,
            id,
            "conversation_create",
            json!({
                "operationId":collaboration_protocol::OperationId::generate(),
                "endpoint":endpoint,"workingDirectory":"/tmp", "fork":null,
                "model":"gpt-5.6-sol", "effort":"low", "access":"workspace-write",
                "createdBy":created_by, "approver":approver, "rootMessageId":null
            }),
        )
        .await;
        if missing_creator {
            assert_eq!(response["result"]["isError"], true);
            assert!(
                response["result"]["content"][0]["text"]
                    .as_str()
                    .is_some_and(|message| message.contains("expected struct SessionRef"))
            );
            assert!(response["result"].get("structuredContent").is_none());
            continue;
        }
        let failure = &response["result"]["structuredContent"];
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(failure["target"], Value::Null);
        assert_eq!(failure["effect"], "none", "{response}");
        assert_eq!(failure["stage"], "validation");
    }

    let response = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        4,
        "conversation_create",
        json!({
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":endpoint,"workingDirectory":"/tmp", "fork":null,
            "model":"gpt-5.6-sol", "effort":"low", "access":"workspace-write",
            "createdBy":local_identity, "approver":local_identity, "rootMessageId":null
        }),
    )
    .await;
    let failure = &response["result"]["structuredContent"];
    assert_eq!(failure["effect"], "none");
    assert_eq!(failure["stage"], "manifest-read");

    let wake_response = mcp_call(
        &client,
        &listener.local_url(),
        &session_id,
        5,
        "wake_wait_until_first_fire",
        json!({"wakeupId":"01985b1e-8d90-7fff-8000-000000000099"}),
    )
    .await;
    let wake_failure = &wake_response["result"]["structuredContent"];
    assert_eq!(
        wake_failure["kind"], "connectionUnavailable",
        "{wake_response}"
    );
    assert_eq!(wake_failure["effect"], "none");
    listener.shutdown().await.expect("MCP shutdown");
}
