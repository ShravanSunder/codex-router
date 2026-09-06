use communication_client::{ClientError, ControlClient};
use communication_protocol::{CodexGeneration, EndpointDescription, SessionRef};
use communication_service::{
    NativeControlBackend, NativeGenerationGate, ServiceIdentity, serve_control_connection,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt, sync::Arc};
use tokio_tungstenite::tungstenite::Message;

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
    if let Some(communication_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
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
    let identity = ServiceIdentity::new(service_id, epoch, &format!("sha256:{}", "a".repeat(64)))
        .unwrap_or_else(|error| panic!("identity: {error}"))
        .with_endpoints(vec![description.clone()])
        .unwrap_or_else(|error| panic!("endpoints: {error}"))
        .with_native_backend(NativeControlBackend {
            codex_home: root.clone(),
            endpoint: target.endpoint.clone(),
            gate,
        })
        .unwrap_or_else(|error| panic!("binding: {error}"));
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let service = tokio::spawn(serve_control_connection(server, identity.clone()));
    let backend = tokio::spawn(async move {
        for (method, expected, result) in [
            (
                "thread/read",
                json!({"threadId":"proof-thread","includeTurns":false}),
                json!({"thread":{"id":"proof-thread","cwd":"/tmp","status":{"type":"idle"}}}),
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
                "turn/interrupt",
                json!({"threadId":"proof-thread","turnId":"proof-turn"}),
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
                    let response = if expected_method == method {
                        result.clone()
                    } else if expected_method == "thread/read" {
                        json!({"thread":{"id":"proof-thread","cwd":"/tmp","status":{"type":"idle"}}})
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
    let message = client
        .send_agent_message(communication_protocol::NativeSendParams {
            target: target.clone(),
            generation: generation.clone(),
            message: communication_protocol::MessageContent::Agent {
                sender: target.clone(),
                text: "A checked finding".to_owned().try_into().unwrap(),
            },
            delivery: communication_protocol::MessageDelivery::Auto,
            client_user_message_id: Some("caller-correlation".to_owned().try_into().unwrap()),
        })
        .await
        .unwrap();
    assert!(matches!(
        message.acceptance,
        communication_protocol::NativeSendAcceptance::NativeInputAccepted {
            disposition: communication_protocol::NativeInputDisposition::StartedOrSteered,
            ..
        }
    ));
    let queued = client
        .send_agent_message(communication_protocol::NativeSendParams {
            target: target.clone(),
            generation: generation.clone(),
            message: communication_protocol::MessageContent::Agent {
                sender: target.clone(),
                text: "A checked finding".to_owned().try_into().unwrap(),
            },
            delivery: communication_protocol::MessageDelivery::Queue,
            client_user_message_id: Some("caller-correlation".to_owned().try_into().unwrap()),
        })
        .await
        .unwrap();
    assert!(
        matches!(queued.acceptance, communication_protocol::NativeSendAcceptance::QueueAccepted { submission_id } if String::from(submission_id.clone()) == "queued-item")
    );
    let inventory = client
        .list_sessions(communication_protocol::NativeSessionListParams {
            endpoint: target.endpoint.clone(),
            view: communication_protocol::NativeSessionView::Loaded,
            page_size: 1,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(inventory.sessions.len(), 1);
    assert_eq!(inventory.sessions[0].target, target);
    assert_eq!(inventory.generation.as_ref(), Some(&generation));
    let interruption = client
        .interrupt_turn(&target, &generation, "proof-turn")
        .await
        .unwrap_or_else(|error| panic!("interrupt: {error}"));
    backend
        .await
        .unwrap_or_else(|error| panic!("backend: {error}"));
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
    if let Some(communication_protocol::ChannelDescription::NativeCodex { schema_digest, .. }) =
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
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert.
    assert_eq!(inspection.target, target);
    assert_eq!(inspection.generation, generation);
    assert_eq!(
        interruption.kind,
        communication_protocol::NativeInterruptKind::InterruptCompleted
    );
    assert!(
        matches!(rejected, Err(ClientError::Rejected { data: Some(data), .. }) if data["kind"] == "staleGeneration")
    );
    assert!(
        matches!(unsupported, Err(ClientError::Rejected { data: Some(data), .. }) if data["kind"] == "unsupportedCapability")
    );
}
