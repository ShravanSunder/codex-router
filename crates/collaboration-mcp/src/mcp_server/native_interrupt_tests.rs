use futures_util::StreamExt;
use serde_json::{Value, json};

const FIXTURE_SERVICE_ID: &str = "00000000-0000-4000-8000-000000000011";
const FIXTURE_SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000012";
const MCP_INTERRUPT_FIXTURE_CONTROL_DIGEST: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

// A failed loopback MCP exchange must abort the real transport proof.
#[allow(clippy::expect_used, clippy::panic)]
async fn send_interrupt_fixture_mcp_request(
    client: &reqwest::Client,
    listener: &crate::CollaborationMcpListener,
    session: &reqwest::header::HeaderValue,
    request_id: u64,
    method: &str,
    params: Value,
) -> Value {
    let response = client
        .post(listener.local_url())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("mcp-session-id", session.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc":"2.0","id":request_id,"method":method,"params":params
        }))
        .send()
        .await
        .expect("MCP HTTP request")
        .error_for_status()
        .expect("MCP HTTP status");
    let body = response.text().await.expect("MCP HTTP response body");
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.is_empty())
        .unwrap_or(&body);
    serde_json::from_str(encoded)
        .unwrap_or_else(|error| panic!("MCP protocol JSON: {error}; {body:?}"))
}

// Malformed or missing native frames must fail the MCP propagation scenario.
#[allow(clippy::expect_used, clippy::panic)]
async fn read_mcp_interrupt_fixture_frame(
    socket: &mut tokio_tungstenite::WebSocketStream<tokio::net::UnixStream>,
) -> Value {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), socket.next())
        .await
        .expect("native fixture frame deadline")
        .expect("native fixture frame")
        .unwrap_or_else(|error| panic!("native fixture receive: {error}"));
    serde_json::from_str(
        frame
            .to_text()
            .unwrap_or_else(|error| panic!("native fixture text: {error}")),
    )
    .unwrap_or_else(|error| panic!("native fixture JSON: {error}"))
}

#[tokio::test]
async fn interrupt_refusals_cross_real_control_service_and_streamable_http_mcp_schema() {
    use collaboration_service::{
        LocalControlService, ManifestPublication, NativeControlBackend, NativeGenerationGate,
        ServiceIdentity,
    };
    use futures_util::SinkExt;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_util::sync::CancellationToken;

    let workspace_tmp = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tmp");
    std::fs::create_dir_all(&workspace_tmp).expect("workspace tmp directory");
    let workspace_tmp = std::fs::canonicalize(workspace_tmp).expect("canonical workspace tmp");
    let temporary = tempfile::Builder::new()
        .prefix("u2-")
        .tempdir_in(workspace_tmp)
        .expect("private service directory");
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private service permissions");
    let native_directory = temporary.path().join("n");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&native_directory)
        .expect("private native socket parent");

    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":FIXTURE_SERVICE_EPOCH,"generation":1
    }))
    .expect("native generation");
    let target: collaboration_protocol::SessionRef = serde_json::from_value(json!({
        "endpoint":{"serviceId":FIXTURE_SERVICE_ID,"endpointId":"codex-local"},
        "sessionId":"mcp-interrupt-thread"
    }))
    .expect("interrupt target");
    let native_socket_path = native_directory.join("codex-native.sock");
    let native_listener =
        tokio::net::UnixListener::bind(&native_socket_path).expect("native WebSocket listener");

    let mut native_definitions = serde_json::Map::new();
    for operation in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "ThreadTurnsList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
        "ThreadQueueAdd",
        "ThreadSetName",
    ] {
        native_definitions.insert(format!("{operation}Params"), json!({"type":"object"}));
        native_definitions.insert(format!("{operation}Response"), json!({"type":"object"}));
    }
    let native_bundle = codex_native_integration::NativeSchemaBundle::from_documents(
        std::collections::BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&json!({"definitions":{"v2":native_definitions}}))
                .expect("native schema JSON"),
        )]),
    )
    .expect("native schema bundle");
    let native_schemas = std::sync::Arc::new(
        codex_native_integration::NativePayloadSchemas::from_bundle(&native_bundle)
            .expect("native schemas"),
    );
    let native_digest = native_schemas.schema_digest().to_owned();
    let endpoint: collaboration_protocol::EndpointDescription = serde_json::from_value(json!({
        "endpoint":target.endpoint,"label":"MCP interrupt fixture",
        "availability":{"state":"available","observedAt":"2026-10-04T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket",
            "path":"codex-native.sock","schemaDigest":native_digest,"generation":generation}]
    }))
    .expect("native endpoint");
    let gate = NativeGenerationGate::default();
    gate.activate(generation.clone(), native_socket_path, Some(native_schemas))
        .expect("native generation admission");
    let backend = NativeControlBackend {
        endpoint: target.endpoint.clone(),
        gate,
        codex_home: native_directory,
    };
    let identity = ServiceIdentity::new(
        FIXTURE_SERVICE_ID,
        FIXTURE_SERVICE_EPOCH,
        MCP_INTERRUPT_FIXTURE_CONTROL_DIGEST,
    )
    .expect("Control service identity")
    .with_endpoints(vec![endpoint])
    .expect("endpoint publication")
    .with_native_backend(backend)
    .expect("native backend");
    let control = LocalControlService::bind(&temporary.path().join("control.sock"), identity)
        .expect("Control listener");
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_value(json!({
        "version":2,"serviceId":FIXTURE_SERVICE_ID,"serviceEpoch":FIXTURE_SERVICE_EPOCH,
        "machineLabel":"mcp-interrupt-fixture",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":MCP_INTERRUPT_FIXTURE_CONTROL_DIGEST,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        ManifestPublication::publish(temporary.path(), &manifest).expect("manifest publication");
    let control_shutdown = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_shutdown.clone()));

    let native_peer = tokio::spawn(async move {
        let cases = [
            ("mcp-busy", -32000, "thread has an active turn"),
            ("mcp-unknown", -32099, "native host refused this turn"),
        ];
        let mut observed_requests = Vec::new();
        for (expected_turn_id, code, message) in cases {
            let (stream, _) =
                tokio::time::timeout(std::time::Duration::from_secs(3), native_listener.accept())
                    .await
                    .expect("native accept deadline")
                    .expect("native accept");
            let mut socket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("native WebSocket upgrade");
            let initialize = read_mcp_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(initialize["method"], "initialize");
            socket
                .send(Message::Text(
                    json!({"id":initialize["id"],"result":{}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("native initialize response");
            let initialized = read_mcp_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(initialized["method"], "initialized");
            let request = read_mcp_interrupt_fixture_frame(&mut socket).await;
            assert_eq!(request["method"], "turn/interrupt");
            assert_eq!(request["params"]["threadId"], "mcp-interrupt-thread");
            assert_eq!(request["params"]["turnId"], expected_turn_id);
            observed_requests.push((
                request["params"]["threadId"]
                    .as_str()
                    .expect("native thread ID")
                    .to_owned(),
                request["params"]["turnId"]
                    .as_str()
                    .expect("native turn ID")
                    .to_owned(),
            ));
            socket
                .send(Message::Text(
                    json!({"id":request["id"],"error":{"code":code,"message":message}})
                        .to_string()
                        .into(),
                ))
                .await
                .expect("native explicit refusal");
        }
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(250),
                native_listener.accept(),
            )
            .await
            .is_err(),
            "MCP interruption refusals must not be replayed"
        );
        observed_requests
    });

    let mcp_listener =
        crate::CollaborationMcpListener::start(crate::CollaborationMcpListenerConfig {
            bind_address: crate::LoopbackBindAddress::parse("127.0.0.1:0")
                .expect("loopback ephemeral bind"),
            service_directory: temporary.path().to_owned(),
            allowed_origins: Vec::new(),
        })
        .await
        .expect("Streamable HTTP MCP listener");
    let http_client = reqwest::Client::new();
    let initialize = http_client
        .post(mcp_listener.local_url())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&json!({
            "jsonrpc":"2.0","id":1,"method":"initialize",
            "params":{"protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"u2-interrupt-proof","version":"1"}}
        }))
        .send()
        .await
        .expect("MCP initialize request");
    let mcp_session = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session ID");
    let initialize_body = initialize.text().await.expect("MCP initialize body");
    assert!(initialize_body.contains("2025-11-25"), "{initialize_body}");
    let initialized = http_client
        .post(mcp_listener.local_url())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header("mcp-session-id", mcp_session.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc":"2.0","method":"notifications/initialized","params":{}
        }))
        .send()
        .await
        .expect("MCP initialized notification");
    assert!(initialized.status().is_success());

    let tools = send_interrupt_fixture_mcp_request(
        &http_client,
        &mcp_listener,
        &mcp_session,
        2,
        "tools/list",
        json!({}),
    )
    .await;
    let interrupt_tool = tools["result"]["tools"]
        .as_array()
        .expect("advertised MCP tool list")
        .iter()
        .find(|tool| tool["name"] == "turn_interrupt")
        .expect("advertised turn_interrupt tool");
    let advertised_output_schema = interrupt_tool["outputSchema"].clone();
    let output_validator = jsonschema::validator_for(&advertised_output_schema)
        .expect("advertised turn_interrupt output schema");

    for (request_id, turn_id, expected_reason, expected_action, expected_code, expected_message) in [
        (
            3,
            "mcp-busy",
            "busy",
            "useDeliverySteer",
            None,
            "thread has an active turn",
        ),
        (
            4,
            "mcp-unknown",
            "unknown",
            "retryLater",
            Some(-32099),
            "native host refused this turn",
        ),
    ] {
        let response = send_interrupt_fixture_mcp_request(
            &http_client,
            &mcp_listener,
            &mcp_session,
            request_id,
            "tools/call",
            json!({"name":"turn_interrupt","arguments":{
                "target":target,
                "generation":generation,
                "turnId":turn_id
            }}),
        )
        .await;
        assert_eq!(response["result"]["isError"], true, "{response}");
        let structured = response["result"]["structuredContent"].clone();
        assert!(
            output_validator.is_valid(&structured),
            "actual MCP structured output violates its advertised schema: {structured}"
        );
        assert_eq!(structured["mcpResult"], "error");
        assert_eq!(structured["kind"], "rejected");
        assert_eq!(structured["serviceKind"], "nativeRejected");
        assert_eq!(structured["stage"], "interrupt");
        assert_eq!(structured["effect"], "none");
        assert_eq!(structured["message"], expected_message);
        assert_eq!(structured["data"]["kind"], "nativeRejected");
        assert_eq!(structured["data"]["stage"], "interrupt");
        assert_eq!(structured["data"]["message"], expected_message);
        assert_eq!(structured["data"]["reason"], expected_reason);
        assert_eq!(structured["data"]["nextAction"], expected_action);
        match expected_code {
            Some(code) => assert_eq!(structured["data"]["nativeCode"], code),
            None => assert!(structured["data"].get("nativeCode").is_none()),
        }
    }

    let observed_requests = native_peer.await.expect("native peer task");
    assert_eq!(
        observed_requests,
        vec![
            ("mcp-interrupt-thread".to_owned(), "mcp-busy".to_owned()),
            ("mcp-interrupt-thread".to_owned(), "mcp-unknown".to_owned()),
        ]
    );
    mcp_listener
        .shutdown()
        .await
        .expect("Streamable HTTP MCP shutdown");
    drop(publication);
    control_shutdown.cancel();
    control_task
        .await
        .expect("Control service task")
        .expect("Control service shutdown");
}
