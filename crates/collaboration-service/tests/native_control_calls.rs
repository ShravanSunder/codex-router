use collaboration_client::{ClientError, ControlClient};
use collaboration_protocol::{CodexGeneration, EndpointDescription, SessionRef};
use collaboration_service::{
    CodexAppServerDeliveryRoute, NativeControlBackend, NativeGenerationGate, ServiceIdentity,
    SessionDeliveryRoute, SessionDeliveryRouter, SessionMessageDelivery, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc};
use tokio_tungstenite::tungstenite::Message;

/// A fixed instant well in the past, so a computed idle time can only be positive.
const BUSY_THREAD_UPDATED_AT_SECONDS: i64 = 1_700_000_000;

#[tokio::test]
async fn sdk_inspection_and_exact_interrupt_use_native_backend_with_generation_guards() {
    // Arrange: isolated Control and backend sockets plus explicitly fixture-only schemas.
    let root = std::path::PathBuf::from(format!("/tmp/native-control-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let backend_path = root.join("backend.sock");
    let listener = tokio::net::UnixListener::bind(&backend_path)
        .unwrap_or_else(|error| panic!("backend: {error}"));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let generation: CodexGeneration =
        serde_json::from_value(json!({"serviceEpoch":epoch,"generation":1}))
            .unwrap_or_else(|error| panic!("generation: {error}"));
    let target: SessionRef = serde_json::from_value(json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"proof-thread"})).unwrap_or_else(|error| panic!("target: {error}"));
    let mut description: EndpointDescription = serde_json::from_value(json!({"endpoint":target.endpoint,"label":"Fixture Codex","availability":{"state":"available","observedAt":"2026-09-05T12:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":generation}]})).unwrap_or_else(|error| panic!("description: {error}"));
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadSetName",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
            .unwrap_or_else(|error| panic!("schema: {error}")),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let digest = format!(
        "sha256:{}",
        bundle
            .digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    if let Some(collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
        description.channels.first_mut()
    {
        *schema_digest = Some(
            digest
                .try_into()
                .unwrap_or_else(|error| panic!("digest: {error}")),
        );
    }
    let schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .unwrap_or_else(|error| panic!("schemas: {error}")),
    );
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), backend_path.clone(), Some(schemas))
        .unwrap_or_else(|error| panic!("activate: {error}"));
    // A recorded Router route makes inspect report the access Router selected.
    let routes_path = root.join("approval-routes.json");
    std::fs::write(
        &routes_path,
        serde_json::to_vec(&json!([{
            "threadId":"proof-thread",
            "createdBy":target,
            "approver":target,
            "access":"workspace-write",
            "scratchPath":"/tmp/native-control-scratch",
            "rootMessageId":null
        }]))
        .unwrap_or_else(|error| panic!("routes: {error}")),
    )
    .unwrap_or_else(|error| panic!("routes file: {error}"));
    let broker = collaboration_service::ServiceApprovalBroker::load(
        service_id
            .to_owned()
            .try_into()
            .unwrap_or_else(|error| panic!("service id: {error}")),
        NativeControlBackend {
            codex_home: root.clone(),
            endpoint: target.endpoint.clone(),
            gate: gate.clone(),
        },
        routes_path.clone(),
    )
    .await
    .unwrap_or_else(|error| panic!("broker: {error}"));
    let native_backend = NativeControlBackend {
        codex_home: root.clone(),
        endpoint: target.endpoint.clone(),
        gate,
    };
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .unwrap_or_else(|error| panic!("identity: {error}"))
        .with_endpoints(vec![description.clone()])
        .unwrap_or_else(|error| panic!("endpoints: {error}"))
        .with_native_backend(native_backend.clone())
        .unwrap_or_else(|error| panic!("binding: {error}"))
        .with_approval_broker(broker);
    let route: Arc<dyn SessionDeliveryRoute> = Arc::new(CodexAppServerDeliveryRoute::new(
        service_id
            .to_owned()
            .try_into()
            .unwrap_or_else(|error| panic!("service id: {error}")),
        identity.endpoint_directory(),
        native_backend,
        Arc::new(collaboration_service::UnmaterializedThreadHolder::new()),
    ));
    let delivery: Arc<dyn SessionMessageDelivery> =
        Arc::new(SessionDeliveryRouter::new(vec![route]));
    let identity = identity.with_session_delivery(delivery);
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let service = tokio::spawn(serve_control_connection(server, identity.clone()));
    // The native Thread schema requires both timestamps as unix seconds.
    let thread_updated_at = chrono::Utc::now().timestamp() - 45;
    let thread_created_at = thread_updated_at - 600;
    let backend = tokio::spawn(async move {
        let mut current_name = "Old name".to_owned();
        for (method, expected, result) in [
            (
                "thread/read",
                json!({"threadId":"proof-thread","includeTurns":false}),
                json!({"thread":{"id":"proof-thread","cwd":"/tmp","status":{"type":"idle"},"createdAt":thread_created_at,"updatedAt":thread_updated_at,"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request","approvalsReviewer":"auto_review"}}),
            ),
            (
                "turn/start",
                json!({"threadId":"proof-thread"}),
                json!({"turn":{"id":"message-turn"}}),
            ),
            (
                "thread/queue/add",
                json!({"threadId":"proof-thread"}),
                json!({"queuedSubmission":{"id":"queued-item"}}),
            ),
            (
                "thread/loaded/list",
                json!({"limit":1,"cursor":null}),
                json!({"data":["proof-thread"],"nextCursor":null}),
            ),
            (
                "thread/name/set",
                json!({"threadId":"proof-thread","name":"🔎 Review"}),
                json!({}),
            ),
            (
                "thread/loaded/list",
                json!({"limit":1,"cursor":null}),
                json!({"data":["proof-thread"],"nextCursor":null}),
            ),
            (
                "turn/interrupt",
                json!({"threadId":"proof-thread","turnId":"proof-turn"}),
                json!({}),
            ),
            (
                "thread/name/set",
                json!({"threadId":"proof-thread","name":"After send"}),
                json!({}),
            ),
        ] {
            let (stream, _) = listener
                .accept()
                .await
                .unwrap_or_else(|error| panic!("accept: {error}"));
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .unwrap_or_else(|error| panic!("upgrade: {error}"));
            let steps = if matches!(method, "turn/start" | "thread/queue/add") {
                vec!["initialize", "initialized", "thread/read", method]
            } else if method == "thread/loaded/list" {
                vec!["initialize", "initialized", method, "thread/read"]
            } else if method == "thread/name/set" {
                if expected["name"] == "After send" {
                    vec!["initialize", "initialized", "thread/read", method]
                } else {
                    vec![
                        "initialize",
                        "initialized",
                        "thread/read",
                        method,
                        "thread/read",
                    ]
                }
            } else {
                vec!["initialize", "initialized", method]
            };
            for expected_method in steps {
                let frame = socket
                    .next()
                    .await
                    .unwrap_or_else(|| panic!("frame"))
                    .unwrap_or_else(|error| panic!("receive: {error}"));
                let request: Value = serde_json::from_str(
                    frame
                        .to_text()
                        .unwrap_or_else(|error| panic!("text: {error}")),
                )
                .unwrap_or_else(|error| panic!("JSON: {error}"));
                assert_eq!(request["method"], expected_method);
                if expected_method == method {
                    if method == "thread/name/set" {
                        current_name = expected["name"]
                            .as_str()
                            .unwrap_or_else(|| panic!("expected rename name"))
                            .to_owned();
                    }
                    if matches!(method, "turn/start" | "thread/queue/add") {
                        assert_eq!(request["params"]["threadId"], expected["threadId"]);
                        assert_eq!(
                            request["params"]["clientUserMessageId"],
                            "caller-correlation"
                        );
                        let text = request["params"]["input"][0]["text"].as_str().unwrap();
                        assert!(text.starts_with("Agent communication\nSelf-declared sender: "));
                        assert!(text.contains("\nIntended recipient: "));
                        assert!(text.ends_with("\n\nA checked finding"));
                    } else {
                        assert_eq!(request["params"], expected);
                    }
                }
                if expected_method != "initialized" {
                    if method == "thread/name/set"
                        && expected["name"] == "After send"
                        && expected_method == method
                    {
                        socket
                            .close(None)
                            .await
                            .unwrap_or_else(|error| panic!("close lost rename socket: {error}"));
                        break;
                    }
                    let response = if expected_method == method {
                        result.clone()
                    } else if expected_method == "thread/read" {
                        // Only the inventory read models a busy thread; the message paths
                        // branch on status and must keep their idle fixture.
                        let status = if method == "thread/loaded/list" {
                            json!({"type":"active","activeFlags":[]})
                        } else {
                            json!({"type":"idle"})
                        };
                        json!({"thread":{"id":"proof-thread","name":current_name,"cwd":"/tmp","status":status,"updatedAt":BUSY_THREAD_UPDATED_AT_SECONDS,"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request","approvalsReviewer":"auto_review"}})
                    } else {
                        json!({})
                    };
                    socket
                        .send(Message::Text(
                            json!({"id":request["id"],"result":response})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .unwrap_or_else(|error| panic!("send: {error}"));
                }
            }
        }
    });
    // Act: no request is sent for the empty sentinel; native inspection, message submission, queueing and interruption follow.
    let mut client = ControlClient::initialize(client, "proof", "1")
        .await
        .unwrap_or_else(|error| panic!("initialize: {error}"));
    assert!(
        client
            .interrupt_turn(&target, &generation, "")
            .await
            .is_err()
    );
    let inspection = client
        .inspect_session(&target)
        .await
        .unwrap_or_else(|error| panic!("inspect: {error}"));
    assert_eq!(
        inspection.effective_access,
        Some(collaboration_protocol::RouterAccess::WorkspaceWrite),
        "inspect reports the access recorded for this thread"
    );
    assert!(matches!(
        inspection.settings_observation,
        collaboration_protocol::SettingsObservation::Unavailable {
            reason: collaboration_protocol::SettingsUnavailableReason::ThreadReadOmitsSettings
        }
    ));
    let message = client
        .send_agent_message(collaboration_protocol::SessionMessageSendParams {
            target: target.clone(),
            generation_guard: Some(generation.clone()),
            message: collaboration_protocol::MessageContent::Agent {
                sender: target.clone(),
                text: "A checked finding".to_owned().try_into().unwrap(),
            },
            mode: collaboration_protocol::MessageDelivery::Auto,
            correlation: Some("caller-correlation".to_owned().try_into().unwrap()),
        })
        .await
        .unwrap();
    assert!(matches!(
        message.client,
        Some(
            collaboration_protocol::DeliveryClientReceipt::CodexAppServer(
                collaboration_protocol::NativeSendReceipt {
                    acceptance: collaboration_protocol::NativeSendAcceptance::NativeInputAccepted {
                        disposition:
                            collaboration_protocol::NativeInputDisposition::StartedOrSteered,
                        ..
                    },
                    ..
                }
            )
        )
    ));
    let queued = client
        .send_agent_message(collaboration_protocol::SessionMessageSendParams {
            target: target.clone(),
            generation_guard: Some(generation.clone()),
            message: collaboration_protocol::MessageContent::Agent {
                sender: target.clone(),
                text: "A checked finding".to_owned().try_into().unwrap(),
            },
            mode: collaboration_protocol::MessageDelivery::Queue,
            correlation: Some("caller-correlation".to_owned().try_into().unwrap()),
        })
        .await
        .unwrap();
    assert!(
        matches!(queued.client, Some(collaboration_protocol::DeliveryClientReceipt::CodexAppServer(collaboration_protocol::NativeSendReceipt { acceptance: collaboration_protocol::NativeSendAcceptance::QueueAccepted { submission_id }, .. })) if String::from(submission_id.clone()) == "queued-item")
    );
    let inventory = client
        .list_sessions(collaboration_protocol::NativeSessionListParams {
            endpoint: target.endpoint.clone(),
            view: collaboration_protocol::NativeSessionView::Loaded,
            scope: collaboration_protocol::NativeSessionScope::Any,
            source: collaboration_protocol::NativeSessionSource::All,
            query: None,
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(inventory.sessions.len(), 1);
    assert_eq!(inventory.sessions[0].target, target);
    assert_eq!(inventory.generation.as_ref(), Some(&generation));
    // A busy row reports the running status and a real idle time, never a hardcoded zero.
    let collaboration_protocol::NativeSessionObservation::Runtime { status, turn_id } =
        &inventory.sessions[0].observation
    else {
        panic!("a loaded row must carry a runtime observation");
    };
    assert_eq!(
        serde_json::to_value(status).unwrap_or_else(|error| panic!("status: {error}"))["type"],
        "active"
    );
    // ActiveThreadStatus in codex_app_server_protocol.v2.schemas.json names no turn id.
    assert_eq!(turn_id.as_deref(), None);
    assert!(
        inventory.sessions[0].idle_seconds > 0,
        "idle seconds must be computed from updatedAt, not hardcoded"
    );
    let renamed = client
        .rename_session(collaboration_protocol::NativeRenameParams {
            target: target.clone(),
            name: "🔎 Review".into(),
        })
        .await
        .unwrap_or_else(|error| panic!("rename: {error}"));
    assert_eq!(renamed.name, "🔎 Review");
    assert_eq!(renamed.previous_name.as_deref(), Some("Old name"));
    let inventory_after_rename = client
        .list_sessions(collaboration_protocol::NativeSessionListParams {
            endpoint: target.endpoint.clone(),
            view: collaboration_protocol::NativeSessionView::Loaded,
            scope: collaboration_protocol::NativeSessionScope::Any,
            source: collaboration_protocol::NativeSessionSource::All,
            query: None,
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap_or_else(|error| panic!("sessions list after rename: {error}"));
    let interruption = client
        .interrupt_turn(&target, &generation, "proof-turn")
        .await
        .unwrap_or_else(|error| panic!("interrupt: {error}"));
    let mut stale = generation.clone();
    stale.generation = 2_u64
        .try_into()
        .unwrap_or_else(|error| panic!("generation: {error}"));
    let rejected = client.interrupt_turn(&target, &stale, "proof-turn").await;
    client
        .close()
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    service
        .await
        .unwrap_or_else(|error| panic!("service: {error}"))
        .unwrap_or_else(|error| panic!("serve: {error}"));
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("lost rename pair: {error}"));
    let service = tokio::spawn(serve_control_connection(server, identity.clone()));
    let mut client = ControlClient::initialize(client, "lost-rename-proof", "1")
        .await
        .unwrap_or_else(|error| panic!("lost rename initialization: {error}"));
    let lost_rename = client
        .rename_session(collaboration_protocol::NativeRenameParams {
            target: target.clone(),
            name: "After send".into(),
        })
        .await;
    client
        .close()
        .await
        .unwrap_or_else(|error| panic!("lost rename close: {error}"));
    service
        .await
        .unwrap_or_else(|error| panic!("lost rename service join: {error}"))
        .unwrap_or_else(|error| panic!("lost rename service: {error}"));
    backend
        .await
        .unwrap_or_else(|error| panic!("backend: {error}"));
    if let Some(collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
        description.channels.first_mut()
    {
        *schema_digest = None;
    }
    identity
        .endpoint_directory()
        .publish(description)
        .unwrap_or_else(|error| panic!("schema withdrawal: {error}"));
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("second pair: {error}"));
    let service = tokio::spawn(serve_control_connection(server, identity));
    let mut client = ControlClient::initialize(client, "withdrawn-schema-proof", "1")
        .await
        .unwrap_or_else(|error| panic!("second initialization: {error}"));
    let unsupported = client.inspect_session(&target).await;
    client
        .close()
        .await
        .unwrap_or_else(|error| panic!("second close: {error}"));
    service
        .await
        .unwrap_or_else(|error| panic!("second service: {error}"))
        .unwrap_or_else(|error| panic!("second serve: {error}"));
    std::fs::remove_file(backend_path).unwrap_or_else(|error| panic!("socket cleanup: {error}"));
    std::fs::remove_file(routes_path).unwrap_or_else(|error| panic!("routes cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert.
    assert_eq!(inspection.target, target);
    assert_eq!(inspection.generation, generation);
    assert_eq!(
        interruption.kind,
        collaboration_protocol::NativeInterruptKind::InterruptCompleted
    );
    assert_eq!(
        inventory_after_rename.sessions[0].name.as_deref(),
        Some("🔎 Review")
    );
    assert!(
        matches!(lost_rename, Err(ClientError::Rejected { data: Some(data), .. })
        if data["kind"] == "outcomeUnknown"
            && data["message"].as_str().is_some_and(|message| message.contains("native app-server closed the socket")))
    );
    assert!(
        matches!(rejected, Err(ClientError::Rejected { data: Some(data), .. }) if data["kind"] == "staleGeneration")
    );
    assert!(
        matches!(unsupported, Err(ClientError::Rejected { data: Some(data), .. }) if data["kind"] == "unsupportedCapability")
    );
}

#[tokio::test]
async fn inspect_control_response_preserves_native_fake_rejection_message() {
    let root = std::path::PathBuf::from(format!(
        "/tmp/native-inspect-rejection-{}",
        std::process::id()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let backend_path = root.join("backend.sock");
    let backend_listener = tokio::net::UnixListener::bind(&backend_path)
        .unwrap_or_else(|error| panic!("backend: {error}"));
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let generation: CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":epoch,"generation":1
    }))
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"},
        "sessionId":"unreadable-thread"
    }))
    .unwrap_or_else(|error| panic!("target: {error}"));
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadSetName",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = codex_native_integration::NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
            .unwrap_or_else(|error| panic!("schema: {error}")),
    )]))
    .unwrap_or_else(|error| panic!("bundle: {error}"));
    let digest = format!(
        "sha256:{}",
        bundle
            .digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let mut endpoint: EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"Fixture Codex",
        "availability":{"state":"available","observedAt":"2026-09-25T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock",
            "schemaDigest":digest,"generation":generation}]
    }))
    .unwrap_or_else(|error| panic!("endpoint: {error}"));
    let schemas = Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&bundle)
            .unwrap_or_else(|error| panic!("schemas: {error}")),
    );
    let gate = NativeGenerationGate::default();
    gate.activate(generation, backend_path.clone(), Some(schemas))
        .unwrap_or_else(|error| panic!("activate: {error}"));
    if let Some(collaboration_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
        endpoint.channels.first_mut()
    {
        *schema_digest = Some(
            digest
                .try_into()
                .unwrap_or_else(|error| panic!("digest: {error}")),
        );
    }
    let native_backend = NativeControlBackend {
        codex_home: root.clone(),
        endpoint: target.endpoint.clone(),
        gate,
    };
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .unwrap_or_else(|error| panic!("identity: {error}"))
        .with_endpoints(vec![endpoint])
        .unwrap_or_else(|error| panic!("endpoints: {error}"))
        .with_native_backend(native_backend)
        .unwrap_or_else(|error| panic!("native backend: {error}"));
    let (client_stream, service_stream) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("Control pair: {error}"));
    let service_task = tokio::spawn(serve_control_connection(service_stream, identity));
    let native_task = tokio::spawn(async move {
        let (stream, _) = backend_listener.accept().await.expect("native accept");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("native upgrade");
        loop {
            let message = socket
                .next()
                .await
                .expect("native frame")
                .expect("native receive");
            let request: Value =
                serde_json::from_str(message.to_text().expect("native text")).expect("native JSON");
            match request["method"].as_str() {
                Some("initialize") => {
                    socket
                        .send(Message::Text(
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{}})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .expect("initialize response");
                }
                Some("initialized") => {}
                Some("thread/read") => {
                    assert_eq!(request["params"]["threadId"], "unreadable-thread");
                    let response = json!({"jsonrpc":"2.0","id":request["id"],"error":{
                        "code":-32600,"message":"native thread is unreadable: fixture refusal"
                    }});
                    socket
                        .send(Message::Text(response.to_string().into()))
                        .await
                        .expect("native rejection");
                    break;
                }
                method => panic!("unexpected native request {method:?}"),
            }
        }
    });

    let mut client = ControlClient::initialize(client_stream, "inspect-test", "0.1.37")
        .await
        .unwrap_or_else(|error| panic!("Control initialize: {error}"));
    let result = client.inspect_session(&target).await;

    assert!(
        matches!(result, Err(ClientError::Rejected { code: -32050, ref data })
        if data.as_ref().and_then(|data| data.get("message")).and_then(Value::as_str)
            == Some("native thread is unreadable: fixture refusal"))
    );
    native_task
        .await
        .unwrap_or_else(|error| panic!("native fake: {error}"));
    service_task.abort();
    let _ = service_task.await;
    std::fs::remove_file(backend_path).unwrap_or_else(|error| panic!("socket cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
}
