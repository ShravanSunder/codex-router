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
        "control": {"transport": "unixJsonLines", "path": "control.sock"},
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
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("manifest");
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

#[tokio::test]
async fn real_http_initialization_discovers_typed_tools_without_authentication() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint =
        json!({"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"});
    let identity = collaboration_service::ServiceIdentity::new(
            "00000000-0000-4000-8000-000000000001",
            "00000000-0000-4000-8000-000000000002",
            &digest,
        )
        .expect("service identity")
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":endpoint,
            "label":"MCP reconnect fixture",
            "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
        })).expect("endpoint description")])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(serde_json::json!({
        "version": 2,
        "serviceId": "00000000-0000-4000-8000-000000000001",
        "serviceEpoch": "00000000-0000-4000-8000-000000000002",
        "control": {"transport": "unixJsonLines", "path": "control.sock"},
        "controlSchemaDigest": digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let _publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control_stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_stop.clone()));
    let acp =
        tokio::net::UnixListener::bind(temporary.path().join("acp.sock")).expect("ACP listener");
    let (active_prompt_tx, active_prompt_rx) = tokio::sync::oneshot::channel();
    let acp_peer = tokio::spawn(async move {
        let mut active_prompt_tx = Some(active_prompt_tx);
        for connection_index in 0..3 {
            let (stream, _) = acp.accept().await.expect("ACP accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("ACP initialize read")
                    .expect("ACP initialize frame"),
            )
            .expect("ACP initialize JSON");
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("ACP initialize response");
            let operation: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("ACP operation read")
                    .expect("ACP operation frame"),
            )
            .expect("ACP operation JSON");
            if connection_index == 0 {
                assert_eq!(operation["method"], "session/new");
                writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":operation["id"],"result":{"sessionId":"reconnect-thread"}})).as_bytes()).await.expect("ACP create response");
            } else {
                assert_eq!(operation["method"], "session/load");
                assert_eq!(operation["params"]["sessionId"], "reconnect-thread");
                writer
                    .write_all(
                        format!(
                            "{}\n",
                            json!({"jsonrpc":"2.0","id":operation["id"],"result":{}})
                        )
                        .as_bytes(),
                    )
                    .await
                    .expect("ACP load response");
                let prompt: Value = serde_json::from_str(
                    &lines
                        .next_line()
                        .await
                        .expect("ACP prompt read")
                        .expect("ACP prompt frame"),
                )
                .expect("ACP prompt JSON");
                assert_eq!(prompt["method"], "session/prompt");
                if connection_index == 1 {
                    writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).as_bytes()).await.expect("ACP prompt response");
                } else {
                    active_prompt_tx
                        .take()
                        .expect("active prompt signal")
                        .send(())
                        .expect("signal active prompt");
                    let cancel: Value = serde_json::from_str(
                        &lines
                            .next_line()
                            .await
                            .expect("ACP cancel read")
                            .expect("ACP cancel frame"),
                    )
                    .expect("ACP cancel JSON");
                    assert_eq!(cancel["method"], "session/cancel");
                    writer
                        .write_all(
                            format!(
                                "{}\n",
                                json!({"jsonrpc":"2.0","id":cancel["id"],"result":{}})
                            )
                            .as_bytes(),
                        )
                        .await
                        .expect("ACP cancel response");
                    writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"cancelled","_meta":{"codex-router/nativeInterruption":{"state":"confirmed"}}}})).as_bytes()).await.expect("ACP cancelled prompt response");
                }
            }
        }
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener starts");
    let client = reqwest::Client::new();
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "collaboration-mcp-test", "version": "1"}
            }
        }))
        .send()
        .await
        .expect("initialize response");
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("transport session id");
    let initialize_body = protocol_response_json(initialize).await;
    assert_eq!(
        initialize_body.pointer("/result/serverInfo/name"),
        Some(&json!("codex-router-collaboration"))
    );

    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc":"2.0",
            "method":"notifications/initialized",
            "params":{}
        }))
        .send()
        .await
        .expect("initialized notification response");
    assert!(initialized.status().is_success());

    let mut idle_stream = tokio::net::TcpStream::connect(listener.local_address())
        .await
        .expect("standalone SSE connection");
    idle_stream
            .write_all(
                format!(
                    "GET /mcp HTTP/1.1\r\nHost: {}\r\nAccept: text/event-stream\r\nmcp-session-id: {}\r\nmcp-protocol-version: 2025-11-25\r\nConnection: close\r\n\r\n",
                    listener.local_address(),
                    session_id.to_str().expect("session header")
                )
                .as_bytes(),
            )
            .await
            .expect("standalone SSE request");
    let mut response_prefix = vec![0_u8; 1024];
    let prefix_bytes = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        idle_stream.read(&mut response_prefix),
    )
    .await
    .expect("SSE response starts")
    .expect("SSE response read");
    assert!(
        String::from_utf8_lossy(&response_prefix[..prefix_bytes]).contains("200 OK"),
        "standalone SSE request must reach the initialized rmcp service"
    );

    let tools = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        .send()
        .await
        .expect("tools response");
    assert!(tools.status().is_success());
    let tools_body = protocol_response_json(tools).await;
    assert_eq!(tools_body.pointer("/result/ttlMs"), Some(&json!(0)));
    assert_eq!(
        tools_body.pointer("/result/cacheScope"),
        Some(&json!("private"))
    );
    let tool_names = tools_body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tool array")
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect::<Vec<_>>();
    assert!(tool_names.contains(&"endpoints_list"));
    assert_eq!(tool_names.len(), 95);
    let tools = tools_body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .expect("tool array");
    for (name, phrase) in [
        ("wake_send", "native input acceptance"),
        ("schedule_prepare", "Preparation mutates"),
        ("board_message_post", "Saving the message"),
        ("conversation_create_and_prompt", "advertised client"),
    ] {
        let description = tools
            .iter()
            .find(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|tool| tool.get("description"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing live description for {name}"));
        assert!(
            description.contains(phrase),
            "{name} description missing {phrase:?}: {description}"
        );
    }

    let endpoint_call = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"tools/call",
            "params":{"name":"endpoints_list","arguments":{}}
        }))
        .send()
        .await
        .expect("endpoint tool response");
    assert!(endpoint_call.status().is_success());
    let endpoint_body = protocol_response_json(endpoint_call).await;
    assert_eq!(
        endpoint_body.pointer("/result/isError"),
        Some(&json!(false))
    );
    assert_eq!(
        endpoint_body.pointer("/result/structuredContent/serviceEpoch"),
        Some(&json!("00000000-0000-4000-8000-000000000002"))
    );
    let create = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",session_id.clone()).header("mcp-protocol-version","2025-11-25").json(&json!({
            "jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"conversation_create","arguments":{
                "operationId":collaboration_protocol::OperationId::generate(),
                "endpoint":endpoint,"workingDirectory":temporary.path(),"fork":null,
                "model":"gpt-5.6-luna","effort":"low","access":"workspace-write",
                "createdBy":{"endpoint":endpoint,"sessionId":"mcp-creator"},
                "approver":{"endpoint":endpoint,"sessionId":"mcp-approver"},"rootMessageId":null
            }}
        })).send().await.expect("conversation create response");
    let create_body = protocol_response_json(create).await;
    assert_eq!(
        create_body.pointer("/result/structuredContent/kind"),
        Some(&json!("created"))
    );
    assert!(
        create_body
            .pointer("/result/structuredContent/operationId")
            .is_some()
    );
    let created_target = create_body
        .pointer("/result/structuredContent/target")
        .cloned()
        .expect("created target");
    assert_eq!(created_target["sessionId"], "reconnect-thread");
    let reconnect_initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"initialize",
            "params":{
                "protocolVersion":"2025-11-25",
                "capabilities":{},
                "clientInfo":{"name":"collaboration-mcp-reconnect-test","version":"1"}
            }
        }))
        .send()
        .await
        .expect("reconnect initialize response");
    assert!(reconnect_initialize.status().is_success());
    let reconnect_session_id = reconnect_initialize
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("reconnect transport session id");
    assert_ne!(reconnect_session_id, session_id);
    let reconnect_body = protocol_response_json(reconnect_initialize).await;
    assert_eq!(
        reconnect_body.pointer("/result/serverInfo/name"),
        Some(&json!("codex-router-collaboration")),
        "a new MCP transport initialization remains a transport concern, not a Router conversation replacement"
    );
    let reconnect_initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", reconnect_session_id.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("reconnect initialized");
    assert!(reconnect_initialized.status().is_success());
    let resumed = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",reconnect_session_id.clone()).header("mcp-protocol-version","2025-11-25").json(&json!({
            "jsonrpc":"2.0","id":41,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
                "target":created_target.clone(),"workingDirectory":temporary.path(),
                "requestedBy":created_target.clone(),
                "message":{"kind":"humanUser","text":"resume without replay"},"timeoutSeconds":3
            }}
        })).send().await.expect("reconnect prompt response");
    let resumed_body = protocol_response_json(resumed).await;
    assert_eq!(
        resumed_body.pointer("/result/structuredContent/target/sessionId"),
        Some(&json!("reconnect-thread"))
    );
    let active_client = client.clone();
    let active_url = listener.local_url();
    let active_session_id = reconnect_session_id.clone();
    let active_target = created_target.clone();
    let active_cwd = temporary.path().to_owned();
    let active_prompt = tokio::spawn(async move {
        active_client.post(active_url).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").header("mcp-session-id",active_session_id).header("mcp-protocol-version","2025-11-25").json(&json!({
                "jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
                    "target":active_target.clone(),"workingDirectory":active_cwd,
                    "requestedBy":active_target,
                    "message":{"kind":"humanUser","text":"active shutdown proof"},"timeoutSeconds":60
                }}
            })).send().await
    });
    active_prompt_rx
        .await
        .expect("active bounded operation starts");
    let session_manager = std::sync::Arc::clone(&listener.session_manager);
    assert_eq!(
        session_manager.sessions.read().await.len(),
        2,
        "each initialized HTTP transport owns separate in-memory MCP session state"
    );
    listener.shutdown().await.expect("listener shutdown");
    assert!(
        session_manager.sessions.read().await.is_empty(),
        "listener shutdown retires owned MCP sessions rather than waiting for idle expiry"
    );
    let mut remainder = Vec::new();
    let stream_end = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        idle_stream.read_to_end(&mut remainder),
    )
    .await;
    assert!(
        stream_end.is_ok(),
        "session close must terminate the actual rmcp service/SSE stream"
    );
    let active_result = tokio::time::timeout(std::time::Duration::from_secs(1), active_prompt)
        .await
        .expect("active bounded operation settles during shutdown")
        .expect("active request join");
    assert!(
        active_result.is_err()
            || active_result.is_ok_and(|response| response.status().is_success()),
        "active request either loses the closing transport or returns its cancellation settlement"
    );
    control_stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    acp_peer.await.expect("ACP peer join");
}

#[tokio::test]
async fn initialized_http_resumed_prompt_load_response_loss_retains_target_without_replay() {
    let body = run_initialized_mcp_resumed_prompt_after_load(None).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        }))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("load"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
}

#[tokio::test]
async fn initialized_http_resumed_prompt_load_rejection_retains_target_code_and_data_without_replay()
 {
    let body = run_initialized_mcp_resumed_prompt_after_load(Some(json!({
        "code": -32050,
        "message": "fixture load rejected",
        "data": {"kind":"nativeRejected","stage":"load","reason":"fixture-policy"}
    })))
    .await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&json!({
            "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},
            "sessionId":"resumed-thread"
        }))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("load"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/code"),
        Some(&json!(-32050))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/data"),
        Some(&json!({"kind":"nativeRejected","stage":"load","reason":"fixture-policy"}))
    );
}

async fn run_initialized_mcp_resumed_prompt_after_load(load_error: Option<Value>) -> Value {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let description = serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP resumed load fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![description])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control_stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_stop.clone()));
    let acp =
        tokio::net::UnixListener::bind(temporary.path().join("acp.sock")).expect("ACP listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP initialize read")
                .expect("ACP initialize frame"),
        )
        .expect("ACP initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        writer
            .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes())
            .await
            .expect("ACP initialize response");
        let load: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP load read")
                .expect("ACP load frame"),
        )
        .expect("ACP load JSON");
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["sessionId"], "resumed-thread");
        if let Some(error) = load_error {
            writer
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":load["id"],"error":error})
                    )
                    .as_bytes(),
                )
                .await
                .expect("ACP load rejection");
            assert!(
                lines
                    .next_line()
                    .await
                    .expect("post-rejection read")
                    .is_none(),
                "a rejected load must not prompt or replay"
            );
        } else {
            drop(lines);
            drop(writer);
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(250), acp.accept())
                    .await
                    .is_err(),
                "a lost load response must not trigger a second ACP connection or replay"
            );
        }
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let initialize = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"resumed-load-proof","version":"1"}}}))
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
    let response = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session_id)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"conversation_prompt","arguments":{
            "target":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"resumed-thread"},
            "workingDirectory":temporary.path(),
            "requestedBy":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"resumed-thread"},
            "message":{"kind":"humanUser","text":"resume proof"},"timeoutSeconds":3
        }}}))
        .send()
        .await
        .expect("resumed prompt response");
    let body = protocol_response_json(response).await;
    listener.shutdown().await.expect("listener shutdown");
    peer.await.expect("ACP peer join");
    control_stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
    body
}

#[tokio::test]
async fn initialized_http_message_response_loss_retains_known_target() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let manifest = serde_json::from_value(json!({
            "version":2,"serviceId":service_id,"serviceEpoch":epoch,
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        for method in ["control/initialize", "message/send"] {
            let request: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("Control read")
                    .expect("Control frame"),
            )
            .expect("Control JSON");
            assert_eq!(request["method"], method);
            if method == "message/send" {
                assert_eq!(request["params"]["target"]["sessionId"], "http-thread");
                break;
            }
            let result = json!({"version":{"major":1,"minor":0},"serviceId":service_id,"serviceEpoch":epoch,"controlSchemaDigest":digest});
            write
                .write_all(
                    format!(
                        "{}\n",
                        json!({"jsonrpc":"2.0","id":request["id"],"result":result})
                    )
                    .as_bytes(),
                )
                .await
                .expect("Control response");
        }
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let initialize = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"message-proof","version":"1"}}}))
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
    let result = client
            .post(listener.local_url())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", session_id)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"message_send","arguments":{
                "target":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"http-thread"},
                "message":{"kind":"humanUser","text":"proof"},"delivery":"auto","generationGuard":null,"correlation":null
            }}}))
            .send()
            .await
            .expect("message response");
    let body = protocol_response_json(result).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target/sessionId"),
        Some(&json!("http-thread"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    listener.shutdown().await.expect("listener shutdown");
    peer.await.expect("peer join");
    drop(publication);
}

#[tokio::test]
async fn initialized_http_acp_initialize_response_loss_is_not_replayed() {
    let body = run_initialized_mcp_create_response_loss(InitializeOutcome::Lost, false).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("initialize"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn initialized_http_acp_version_mismatch_has_no_effect_or_creation() {
    let body =
        run_initialized_mcp_create_response_loss(InitializeOutcome::VersionMismatch, false).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("initialize"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn initialized_http_fork_response_loss_is_not_replayed() {
    let body = run_initialized_mcp_create_response_loss(InitializeOutcome::Success, true).await;

    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("fork"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&Value::Null)
    );
}

#[tokio::test]
async fn initialized_http_observation_attach_failure_retains_target_and_pre_dispatch_effect() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"observe-thread"});
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP observation fixture",
            "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
            "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"missing-native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
        })).expect("endpoint description")])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(stop.clone()));
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let session_id = initialize_mcp_session(&client, &listener, "observation-attach-proof").await;
    let response = client.post(listener.local_url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", session_id).header("mcp-protocol-version", "2025-11-25").json(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"events_observe","arguments":{"target":target,"timeoutSeconds":1,"maxEvents":1,"maxBytes":1}}
    })).send().await.expect("observation response");
    let body = protocol_response_json(response).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&target)
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("observation-attach"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("none"))
    );
    listener.shutdown().await.expect("listener shutdown");
    stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
}

#[tokio::test]
async fn initialized_http_observation_resume_response_loss_retains_target_and_unknown_effect() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let target = json!({"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"observe-thread"});
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest).expect("service identity").with_endpoints(vec![serde_json::from_value(json!({
        "endpoint":{"serviceId":service_id,"endpointId":"codex-local"}, "label":"MCP observation resume fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":{"serviceEpoch":epoch,"generation":1}}]
    })).expect("endpoint description")]).expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({"version":2,"serviceId":service_id,"serviceEpoch":epoch,"control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}})).expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(stop.clone()));
    let native = tokio::net::UnixListener::bind(temporary.path().join("native.sock"))
        .expect("native listener");
    let native_peer = tokio::spawn(async move {
        let (stream, _) = native.accept().await.expect("native accept");
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .expect("native websocket");
        let initialize = socket
            .next()
            .await
            .expect("initialize frame")
            .expect("initialize receive");
        let initialize: Value =
            serde_json::from_str(initialize.to_text().expect("initialize text"))
                .expect("initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        use futures_util::SinkExt;
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":initialize["id"],"result":{"userAgent":"observation-fixture"}})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("initialize response");
        let initialized = socket
            .next()
            .await
            .expect("initialized frame")
            .expect("initialized receive");
        assert_eq!(
            serde_json::from_str::<Value>(initialized.to_text().expect("initialized text"))
                .expect("initialized JSON")["method"],
            "initialized"
        );
        let resume = socket
            .next()
            .await
            .expect("resume frame")
            .expect("resume receive");
        let resume: Value =
            serde_json::from_str(resume.to_text().expect("resume text")).expect("resume JSON");
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "observe-thread");
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let session_id =
        initialize_mcp_session(&client, &listener, "observation-resume-loss-proof").await;
    let response = client.post(listener.local_url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", session_id).header("mcp-protocol-version", "2025-11-25").json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"events_observe","arguments":{"target":target,"timeoutSeconds":1,"maxEvents":1,"maxBytes":1}}})).send().await.expect("observation response");
    let body = protocol_response_json(response).await;
    assert_eq!(body.pointer("/result/isError"), Some(&json!(true)));
    assert_eq!(
        body.pointer("/result/structuredContent/target"),
        Some(&target)
    );
    assert_eq!(
        body.pointer("/result/structuredContent/stage"),
        Some(&json!("observation-attach"))
    );
    assert_eq!(
        body.pointer("/result/structuredContent/effect"),
        Some(&json!("unknown"))
    );
    listener.shutdown().await.expect("listener shutdown");
    native_peer.await.expect("native peer join");
    stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
}

#[derive(Clone, Copy)]
enum InitializeOutcome {
    Success,
    Lost,
    VersionMismatch,
}

async fn run_initialized_mcp_create_response_loss(
    initialize_outcome: InitializeOutcome,
    fork: bool,
) -> Value {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o700))
            .expect("private service directory");
    }
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let digest = format!("sha256:{}", "a".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint, "label":"MCP create-loss fixture",
        "availability":{"state":"available","observedAt":"2026-09-19T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch, &digest)
        .expect("service identity")
        .with_endpoints(vec![description])
        .expect("endpoint directory");
    let control = collaboration_service::LocalControlService::bind(
        &temporary.path().join("control.sock"),
        identity,
    )
    .expect("control listener");
    let manifest = serde_json::from_value(json!({
        "version":2,"serviceId":service_id,"serviceEpoch":epoch,
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,"mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .expect("service manifest");
    let publication =
        collaboration_service::ManifestPublication::publish(temporary.path(), &manifest)
            .expect("manifest publication");
    let control_stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(control_stop.clone()));
    let acp =
        tokio::net::UnixListener::bind(temporary.path().join("acp.sock")).expect("ACP listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = acp.accept().await.expect("ACP accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize: Value = serde_json::from_str(
            &lines
                .next_line()
                .await
                .expect("ACP initialize read")
                .expect("ACP initialize frame"),
        )
        .expect("ACP initialize JSON");
        assert_eq!(initialize["method"], "initialize");
        if matches!(initialize_outcome, InitializeOutcome::Lost) {
            drop(writer);
            drop(lines);
        } else {
            let protocol_version =
                if matches!(initialize_outcome, InitializeOutcome::VersionMismatch) {
                    2
                } else {
                    1
                };
            writer.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],"result":{"protocolVersion":protocol_version,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes()).await.expect("ACP initialize response");
            if matches!(initialize_outcome, InitializeOutcome::VersionMismatch) {
                let next =
                    tokio::time::timeout(std::time::Duration::from_millis(250), lines.next_line())
                        .await;
                assert!(
                    !matches!(next, Ok(Ok(Some(_)))),
                    "version mismatch must not dispatch an ACP conversation operation"
                );
                return;
            }
            let create: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("ACP create read")
                    .expect("ACP create frame"),
            )
            .expect("ACP create JSON");
            assert_eq!(create["method"], "session/new");
            assert_eq!(
                create.pointer("/params/_meta/codexRouter/forkThreadId"),
                Some(&json!("fork-source"))
            );
            drop(writer);
            drop(lines);
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(250), acp.accept())
                .await
                .is_err(),
            "a lost ACP response must not create a replacement connection or replay the mutation"
        );
    });
    let listener = CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: temporary.path().to_owned(),
        allowed_origins: vec!["http://localhost".to_owned()],
    })
    .await
    .expect("MCP listener");
    let client = reqwest::Client::new();
    let session_id = initialize_mcp_session(&client, &listener, "create-loss-proof").await;
    let response = client.post(listener.local_url()).header(CONTENT_TYPE, "application/json").header(ACCEPT, "application/json, text/event-stream").header("mcp-session-id", session_id).header("mcp-protocol-version", "2025-11-25").json(&json!({
        "jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"conversation_create","arguments":{
            "operationId":collaboration_protocol::OperationId::generate(),
            "endpoint":endpoint,"workingDirectory":temporary.path(),"fork":if fork { json!("fork-source") } else { Value::Null },
            "model":"gpt-5.6-luna","effort":if fork { Value::Null } else { json!("low") },"access":"workspace-write",
            "createdBy":{"endpoint":endpoint,"sessionId":"mcp-creator"},
            "approver":{"endpoint":endpoint,"sessionId":"mcp-approver"},"rootMessageId":null
        }}
    })).send().await.expect("conversation create response");
    let body = protocol_response_json(response).await;
    listener.shutdown().await.expect("listener shutdown");
    peer.await.expect("ACP peer join");
    control_stop.cancel();
    control_task
        .await
        .expect("control join")
        .expect("control shutdown");
    drop(publication);
    body
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
