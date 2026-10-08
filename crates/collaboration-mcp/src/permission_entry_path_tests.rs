use crate::api_test_harness::{ServedApi, api_config};
use codex_acp_adapter::{
    AcpSchemaCatalog, ApprovalBroker, ApprovalRoute, BrokeredApprovalOutcome,
    BrokeredApprovalRequest, PendingPermission,
};
use collaboration_protocol::{
    ApprovalDecision, CodexGeneration, EndpointDescription, EndpointRef, MachineId, PushLineInput,
    RouterAccess, RouterLink, RouterOriginRef, SessionRef, render_push_line,
};
use collaboration_service::{
    EndpointDirectory, LocalControlService, MachineIdentity, ManifestPublication,
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, ServiceInteractionBroker,
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
    application: collaboration_service::CollaborationApplication,
    broker: Arc<ServiceInteractionBroker>,
    _automation_store: Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>,
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

struct ApprovalDeliveryCapture {
    requester: SessionRef,
    approver: SessionRef,
    automation_store: Arc<tokio::sync::Mutex<automation_storage::AutomationStore>>,
    broker: Arc<ServiceInteractionBroker>,
    machine_identity: MachineIdentity,
}

struct AcceptedTypedNoticeDelivery;

impl collaboration_service::SessionMessageDelivery for AcceptedTypedNoticeDelivery {
    fn deliver<'a>(
        &'a self,
        _: collaboration_service::layer_zero::DeliveryRequest,
        _: &'a dyn collaboration_service::AttemptEvidenceSink,
    ) -> collaboration_service::DeliveryFuture<'a, collaboration_protocol::DeliveryReceipt> {
        Box::pin(async {
            Ok(collaboration_protocol::DeliveryReceipt {
                outcome: collaboration_protocol::DeliveryOutcome::Started,
                reachability: Some(collaboration_protocol::SessionReachability::CodexAppServer),
                client: None,
            })
        })
    }

    fn reconcile_attempt(
        &self,
        _: collaboration_service::AttemptReconciliationContext,
    ) -> collaboration_service::DeliveryFuture<'_, collaboration_service::AttemptReconciliation>
    {
        Box::pin(async { Ok(collaboration_service::AttemptReconciliation::StillUnknown) })
    }
}

impl ApprovalFixture {
    async fn start(delivery_count: usize) -> Self {
        Self::start_with_dropped_decision_reply(delivery_count, false, false).await
    }

    async fn start_typed() -> Self {
        Self::start_with_dropped_decision_reply(0, false, true).await
    }

    async fn start_with_dropped_decision_reply(
        delivery_count: usize,
        drop_decision_reply: bool,
        typed_notice: bool,
    ) -> Self {
        let directory = tempfile::tempdir_in("/tmp").expect("private service directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private permissions");
        }
        let automation_path = directory.path().join("automation.sqlite");
        let automation_store = Arc::new(tokio::sync::Mutex::new(
            automation_storage::AutomationStore::open(&automation_path)
                .await
                .expect("automation store"),
        ));
        let service_id: collaboration_protocol::UuidIdentity =
            SERVICE_ID.to_owned().try_into().expect("service id");
        let machine_identity = MachineIdentity::new(service_id.clone(), Some("Permission fixture"))
            .expect("machine identity");
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
        let broker = ServiceInteractionBroker::load(
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
        let delivery: Arc<dyn collaboration_service::SessionMessageDelivery> = if typed_notice {
            Arc::new(AcceptedTypedNoticeDelivery)
        } else {
            Arc::new(collaboration_service::SessionDeliveryRouter::new(vec![
                route,
            ]))
        };
        broker
            .install_session_delivery(delivery)
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
        let backend_task = tokio::spawn(serve_approval_deliveries(
            backend_listener,
            delivery_count,
            ApprovalDeliveryCapture {
                requester: requester.clone(),
                approver: approver.clone(),
                automation_store: Arc::clone(&automation_store),
                broker: Arc::clone(&broker),
                machine_identity: machine_identity.clone(),
            },
            delivery_tx,
        ));
        let digest = format!("sha256:{}", "a".repeat(64));
        let identity = ServiceIdentity::new(SERVICE_ID, SERVICE_EPOCH)
            .expect("service identity")
            .with_endpoints(vec![description])
            .expect("service endpoint")
            .with_machine_identity(machine_identity)
            .expect("machine identity")
            .with_native_backend(native_backend)
            .expect("native backend")
            .with_automation_store(Arc::clone(&automation_store))
            .with_approval_broker(Arc::clone(&broker));
        let control_path = directory.path().join(if drop_decision_reply {
            "broker-control.sock"
        } else {
            "control.sock"
        });
        let application = collaboration_service::CollaborationApplication::new(identity.clone());
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
            "machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},
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
            application,
            broker,
            _automation_store: automation_store,
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
    capture: ApprovalDeliveryCapture,
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
                    "id":String::from(capture.approver.session_id.clone()), "cwd":"/tmp",
                    "status":{"type":"idle"}, "createdAt":1, "updatedAt":1,
                    "sandbox":{"type":"workspaceWrite"}, "approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review"
                }}),
                "turn/start" => {
                    let text = request["params"]["input"][0]["text"]
                        .as_str()
                        .expect("push delivery line");
                    let (_, link_text) = text.rsplit_once(" · ").expect("push link suffix");
                    let link = RouterLink::parse(link_text).expect("valid router push link");
                    assert_eq!(
                        link.machine_id(),
                        &MachineId::from(capture.machine_identity.service_id().clone())
                    );
                    assert!(!text.contains("Self-declared sender:"));
                    assert!(!text.contains("Intended recipient:"));

                    let push = capture
                        .automation_store
                        .lock()
                        .await
                        .get_push_record(link.push_id())
                        .await
                        .expect("read delivered push")
                        .expect("stored delivered push");
                    assert_eq!(&push.push_id, link.push_id());
                    assert_eq!(push.target, capture.approver);
                    let expected_line = render_push_line(&PushLineInput {
                        link,
                        machine_label: capture.machine_identity.machine_label().clone(),
                        origin: push.origin.clone(),
                        header_facts: push.header_facts.clone(),
                        body: push.body.clone(),
                    })
                    .expect("render stored push line");
                    assert_eq!(text, expected_line);
                    assert_eq!(text.lines().count(), 1);
                    let origin = RouterOriginRef::parse_canonical(
                        push.origin_router_ref
                            .as_deref()
                            .expect("interaction origin reference"),
                    )
                    .expect("canonical interaction origin");
                    let RouterOriginRef::Interaction { interaction_id, .. } = origin else {
                        panic!("interaction origin expected");
                    };
                    let body = push.body.as_deref().expect("stored push body");
                    assert!(body.contains(interaction_id.as_str()));
                    match push.kind {
                        collaboration_protocol::PushKind::Approval => {
                            assert!(text.starts_with("❓ Router approval @Permission fixture"));
                            assert_eq!(
                                push.origin,
                                collaboration_protocol::PushOrigin::Router(
                                    collaboration_protocol::PushKind::Approval,
                                )
                            );
                            assert!(matches!(
                                &push.header_facts,
                                collaboration_protocol::PushHeaderFacts::Approval {
                                    requester: push_requester,
                                    ..
                                } if push_requester == &capture.requester
                            ));
                            assert!(body.contains("Allow this operation once (allow once)"));
                            assert!(body.contains("agent-collaboration approval decide"));
                            let approval = capture
                                .broker
                                .list(false)
                                .await
                                .approvals
                                .into_iter()
                                .find(|record| record.request_id == interaction_id.as_str())
                                .expect("approval associated with delivered push");
                            assert_eq!(approval.requester, capture.requester);
                            assert_eq!(approval.approver, capture.approver);
                            assert_eq!(
                                serde_json::to_value(&approval.generation)
                                    .expect("generation JSON")["generation"],
                                if delivery_count == 2 && index == 1 {
                                    6
                                } else {
                                    7
                                }
                            );
                            assert!(
                                approval.operation["params"]["toolCall"]["content"][0]
                                    ["content"]["text"]
                                    .as_str()
                                    .is_some_and(|text| text
                                        .contains("Requested permissions:")
                                        && text.contains("\"enabled\": true"))
                            );
                        }
                        collaboration_protocol::PushKind::Question => {
                            assert!(text.starts_with("❓ Router question @Permission fixture"));
                            assert_eq!(
                                push.origin,
                                collaboration_protocol::PushOrigin::Router(
                                    collaboration_protocol::PushKind::Question,
                                )
                            );
                            assert!(matches!(
                                &push.header_facts,
                                collaboration_protocol::PushHeaderFacts::Question {
                                    requester: push_requester,
                                    ..
                                } if push_requester == &capture.requester
                            ));
                            assert!(body.contains("Question: Choose settings"));
                            assert!(body.contains("mcp-question"));
                            assert!(body.contains("agent-collaboration question answer"));
                        }
                        _ => panic!("approval or question push expected"),
                    }
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

#[path = "permission_entry_path_tests/real_entry_tests.rs"]
mod real_entry_tests;
