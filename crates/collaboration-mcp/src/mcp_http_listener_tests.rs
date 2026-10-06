use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue, ORIGIN};
use rmcp::transport::streamable_http_server::session::SessionManager;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

#[test]
fn bind_address_rejects_wildcard_and_non_loopback_interfaces() {
    assert!(LoopbackBindAddress::parse("127.0.0.1:0").is_ok());
    assert!(LoopbackBindAddress::parse("[::1]:0").is_ok());
    assert!(LoopbackBindAddress::parse("0.0.0.0:0").is_err());
    assert!(LoopbackBindAddress::parse("192.0.2.10:8080").is_err());
}

#[tokio::test]
async fn dropping_owned_listener_releases_loopback_port() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts");
    let address = listener.local_address();
    let session_manager = std::sync::Arc::clone(&listener.session_manager);
    let (_session_id, _transport) = session_manager
        .create_session()
        .await
        .expect("owned MCP session");
    assert_eq!(session_manager.sessions.read().await.len(), 1);
    drop(listener);
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !session_manager.sessions.read().await.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("drop retires owned MCP session within bounded shutdown");
    let rebound = tokio::net::TcpListener::bind(address)
        .await
        .expect("owned listener port released on drop");
    drop(rebound);
}

#[tokio::test]
async fn reported_listener_failure_can_shutdown_without_repolling_completed_task() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let mut listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts");
    let address = listener.local_address();
    let session_manager = std::sync::Arc::clone(&listener.session_manager);
    let (_session_id, _transport) = session_manager
        .create_session()
        .await
        .expect("owned MCP session");
    listener.accept_task.as_ref().expect("accept task").abort();

    let failure = listener.listener_failure().await;
    assert!(failure.to_string().contains("cancelled"));
    assert!(
        session_manager.sessions.read().await.is_empty(),
        "listener failure retires owned MCP sessions"
    );
    listener
        .shutdown()
        .await
        .expect("shutdown treats the reported task as consumed");
    let rebound = tokio::net::TcpListener::bind(address)
        .await
        .expect("listener failure shutdown releases loopback port");
    drop(rebound);
}

#[tokio::test]
async fn cancelling_an_initialized_mcp_wake_wait_retires_its_call_local_connection() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(serde_json::json!({
        "version": 2,
        "serviceId": "00000000-0000-4000-8000-000000000001",
        "serviceEpoch": "00000000-0000-4000-8000-000000000002",
        "machineLabel":"fixture-host","control": {"transport": "unixJsonLines", "path": "control.sock"},
        "controlSchemaDigest": digest,
        "mcp": {"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let _publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let control_peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("Control initialize read")
                .expect("Control initialize frame"),
        )
        .expect("Control initialize JSON");
        assert_eq!(initialize["method"], "control/initialize");
        writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}})
                    )
                    .as_bytes(),
                )
                .await
                .expect("Control initialize response");
        let subscribe: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("wake subscribe read")
                .expect("wake subscribe frame"),
        )
        .expect("wake subscribe JSON");
        assert_eq!(subscribe["method"], "wake/subscribe");
        assert_eq!(
            subscribe.pointer("/params/wakeupId"),
            Some(&json!("01a0bb62-9a72-7161-8103-b6c2c691bec8"))
        );
        writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":subscribe["id"],"result":{
                            "subscriptionId":"01a0bb62-9a72-7162-8103-b6c2c691bec8",
                            "after":"[1,\\\"00000000-0000-4000-8000-000000000001\\\",\\\"automation-events\\\",0,0]",
                            "snapshot":{
                                "definition":{"wakeupId":"01a0bb62-9a72-7161-8103-b6c2c691bec8","changeId":"01a0bb62-9a72-7163-8103-b6c2c691bec8","message":{"target":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"wake-target"},"content":{"kind":"humanUser","text":"wake fixture"},"delivery":"auto","generationGuard":null},"timing":{"kind":"after","seconds":3600},"anchorAt":"2026-09-19T00:00:00Z","expiresAt":null,"createdAt":"2026-09-19T00:00:00Z"},
                                "state":"active","nextDueAt":"2026-09-19T01:00:00Z","firstFire":null,"pendingDeliveryId":null,"latestEventCursor":"[1,\\\"00000000-0000-4000-8000-000000000001\\\",\\\"automation-events\\\",0,0]"
                            }
                        }})
                    )
                    .as_bytes(),
                )
                .await
                .expect("wake subscribe response");
        subscribed_tx.send(()).expect("wake subscription signal");
        assert!(
            lines.next_line().await.expect("Control EOF read").is_none(),
            "request/session cancellation must close the call-local Control connection without cancelling the durable wake"
        );
        closed_tx.send(()).expect("Control close signal");
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts");
    let client = reqwest::Client::new();
    let initialize = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"wake-cancellation-proof","version":"1"}}}))
            .send()
            .await
            .expect("MCP initialize response");
    assert!(initialize.status().is_success());
    let session_header = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session id");
    let _initialize_body = protocol_response_json(initialize).await;
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_header.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("MCP initialized notification");
    assert!(initialized.status().is_success());
    drop(initialized);
    let active_client = reqwest::Client::new();
    let active_url = listener.local_url();
    let active_session = session_header.clone();
    let mut wake_request = tokio::spawn(async move {
        let response = active_client
                .post(active_url)
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", active_session)
                .header("mcp-protocol-version", "2025-11-25")
                .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"wake_wait_until_first_fire","arguments":{"wakeupId":"01a0bb62-9a72-7161-8103-b6c2c691bec8"}}}))
                .send()
                .await?;
        response.bytes().await
    });
    tokio::select! {
        subscribed = subscribed_rx => {
            subscribed.expect("active wake subscription");
        }
        request = &mut wake_request => {
            panic!("wake request ended before establishing its Control subscription: {request:?}");
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
            panic!("active wake subscription deadline");
        }
    }
    let session_manager = std::sync::Arc::clone(&listener.session_manager);
    let active_services = std::sync::Arc::clone(&listener.active_services);
    let cancellation = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            reqwest::Client::new()
                .post(listener.local_url())
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "application/json, text/event-stream")
                .header("mcp-session-id", session_header)
                .header("mcp-protocol-version", "2025-11-25")
                .json(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"caller stopped waiting"}}))
                .send(),
        )
        .await
        .expect("MCP cancellation notification response deadline")
        .expect("MCP cancellation notification response");
    assert!(cancellation.status().is_success());
    tokio::time::timeout(std::time::Duration::from_secs(1), closed_rx)
        .await
        .expect("call-local Control connection closes")
        .expect("Control peer close signal");
    let _request_result = tokio::time::timeout(std::time::Duration::from_secs(1), wake_request)
        .await
        .expect("cancelled MCP request settles without a promised late response")
        .expect("cancelled MCP request join");
    listener.shutdown().await.expect("listener shutdown");
    assert!(
        session_manager.sessions.read().await.is_empty(),
        "listener shutdown retires the initialized MCP session after request cancellation"
    );
    assert_eq!(
        active_services.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "listener shutdown retires the actual rmcp service lifetime"
    );
    control_peer.await.expect("Control peer join");
}

#[tokio::test]
async fn cancelling_an_initialized_mcp_wake_subscribe_retires_held_connection_without_mutating_wake()
 {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","machineLabel":"fixture-host","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("manifest");
    let _publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let (subscribed_tx, subscribed_rx) = tokio::sync::oneshot::channel();
    let (closed_tx, closed_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("Control initialize read")
                .expect("Control initialize frame"),
        )
        .expect("Control initialize JSON");
        assert_eq!(initialize["method"], "control/initialize");
        writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}})).as_bytes()).await.expect("Control initialize response");
        let subscribe: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("wake subscribe read")
                .expect("wake subscribe frame"),
        )
        .expect("wake subscribe JSON");
        assert_eq!(subscribe["method"], "wake/subscribe");
        subscribed_tx.send(()).expect("held subscribe signal");
        assert!(
            lines.next_line().await.expect("Control EOF read").is_none(),
            "request cancellation must retire the held call-local subscription connection without cancelling the durable wake"
        );
        closed_tx.send(()).expect("Control close signal");
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let session_id = initialize_mcp_session(&client, &listener, "wake-held-subscribe-proof").await;
    let active_client = client.clone();
    let active_url = listener.local_url();
    let active_session = session_id.clone();
    let request = tokio::spawn(async move {
        active_client.post(active_url).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", active_session).header("mcp-protocol-version", "2025-11-25").json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"wake_wait_until_first_fire","arguments":{"wakeupId":"01a0bb62-9a72-7161-8103-b6c2c691bec8"}}})).send().await
    });
    tokio::time::timeout(std::time::Duration::from_secs(1), subscribed_rx)
        .await
        .expect("subscribe deadline")
        .expect("subscribe signal");
    let cancellation = client.post(listener.local_url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", session_id).header("mcp-protocol-version", "2025-11-25").json(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"caller stopped waiting"}})).send().await.expect("MCP cancellation response");
    assert!(cancellation.status().is_success());
    tokio::time::timeout(std::time::Duration::from_secs(1), closed_rx)
        .await
        .expect("held Control connection closes")
        .expect("close signal");
    let _request_result = tokio::time::timeout(std::time::Duration::from_secs(1), request)
        .await
        .expect("cancelled request settles")
        .expect("request join");
    listener.shutdown().await.expect("listener shutdown");
    peer.await.expect("Control peer join");
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let status = response.status();
    let body = response.text().await.expect("protocol response body");
    assert!(
        !body.is_empty(),
        "empty protocol response with status {status}"
    );
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body.as_str());
    serde_json::from_str(encoded)
        .unwrap_or_else(|error| panic!("protocol response JSON: {error}; body={body:?}"))
}

async fn initialize_mcp_session(
    client: &reqwest::Client,
    listener: &CollaborationMcpListener,
    name: &str,
) -> reqwest::header::HeaderValue {
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":name,"version":"1"}}}))
        .send()
        .await
        .expect("MCP initialize");
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session");
    let _body = protocol_response_json(initialize).await;
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("MCP initialized");
    assert!(initialized.status().is_success());
    session_id
}

#[path = "mcp_http_listener_tests/initialization_tests.rs"]
mod initialization_tests;

#[path = "mcp_http_listener_tests/response_loss_tests.rs"]
mod response_loss_tests;

#[tokio::test]
async fn schema_required_fields_reach_each_tool_without_missing_field_errors() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts");
    let client = reqwest::Client::new();
    let session = initialize_mcp_session(&client, &listener, "schema-required-inputs").await;
    let server = crate::mcp_server::CollaborationMcpServer::new(temporary.path().to_owned());
    let definitions = server
        .resolved_tools()
        .into_iter()
        .map(|tool| {
            let schema =
                serde_json::to_value(tool.input_schema.as_ref()).expect("input schema JSON");
            (tool.name.into_owned(), schema)
        })
        .collect::<std::collections::BTreeMap<_, _>>();

    for (index, (name, schema)) in definitions.iter().enumerate() {
        let arguments = schema_required_object(schema, &schema["$defs"]);
        let response = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", session.clone())
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({
                "jsonrpc":"2.0","id":index + 2,"method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("{name} HTTP call: {error}"));
        let response = protocol_response_json(response).await;
        let encoded = response.to_string();
        assert!(
            !encoded.contains("missing field") && !encoded.contains("requires "),
            "{name} rejected the schema-required input as incomplete: {encoded}"
        );
        if name == "wake_send" {
            assert!(
                !encoded.contains("explicit nullable generationGuard"),
                "wake_send must accept an omitted optional generationGuard: {encoded}"
            );
        }
    }
}

fn schema_required_object(schema: &Value, definitions: &Value) -> Value {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference.strip_prefix("#/$defs/").unwrap_or_default();
        return schema_required_object(&definitions[name], definitions);
    }
    if let Some(constant) = schema.get("const") {
        return constant.clone();
    }
    if let Some(variants) = schema.get("enum").and_then(Value::as_array) {
        return variants.first().cloned().unwrap_or(Value::Null);
    }
    for keyword in ["oneOf", "anyOf"] {
        if let Some(variants) = schema.get(keyword).and_then(Value::as_array) {
            return variants
                .first()
                .map(|variant| schema_required_object(variant, definitions))
                .unwrap_or(Value::Null);
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let required = schema["required"].as_array().cloned().unwrap_or_default();
            let mut object = serde_json::Map::new();
            for field in required.iter().filter_map(Value::as_str) {
                object.insert(
                    field.to_owned(),
                    schema_required_object(&schema["properties"][field], definitions),
                );
            }
            Value::Object(object)
        }
        Some("array") => Value::Array(Vec::new()),
        Some("integer") => schema.get("minimum").cloned().unwrap_or(json!(1)),
        Some("number") => schema.get("minimum").cloned().unwrap_or(json!(1)),
        Some("boolean") => Value::Bool(false),
        Some("string") => {
            if schema.get("format").and_then(Value::as_str) == Some("date-time") {
                json!("2026-09-24T00:00:00Z")
            } else if schema
                .get("pattern")
                .and_then(Value::as_str)
                .is_some_and(|pattern| pattern.contains("7[0-9a-f]{3}"))
            {
                json!("019f0000-0000-7000-8000-000000000001")
            } else if schema
                .get("pattern")
                .and_then(Value::as_str)
                .is_some_and(|pattern| pattern.contains("[a-z]"))
            {
                json!("fixture")
            } else if schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) > 0 {
                json!("x")
            } else {
                json!("")
            }
        }
        _ => Value::Null,
    }
}

#[tokio::test]
async fn invalid_origin_is_rejected_before_protocol_dispatch() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener starts");
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/json, text/event-stream"),
    );
    headers.insert(ORIGIN, HeaderValue::from_static("https://invalid.example"));
    let response = reqwest::Client::new()
        .post(listener.local_url())
        .headers(headers)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "invalid-origin", "version": "1"}
            }
        }))
        .send()
        .await
        .expect("origin response");
    assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn real_http_initialization_supports_ipv6_loopback() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("[::1]:0").expect("IPv6 loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("IPv6 MCP listener starts");
    let response = reqwest::Client::new()
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":1,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-11-25",
                "capabilities":{},
                "clientInfo":{"name":"ipv6-proof","version":"1"}
            }
        }))
        .send()
        .await
        .expect("IPv6 initialize response");
    assert!(response.status().is_success());
    assert!(response.headers().contains_key("mcp-session-id"));
    listener.shutdown().await.expect("IPv6 listener shutdown");
}
