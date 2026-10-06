use super::*;

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
        "machineLabel":"fixture-host","control": {"transport": "unixJsonLines", "path": "control.sock"},
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
                    assert!(
                        tokio::time::timeout(std::time::Duration::from_secs(12), lines.next_line())
                            .await
                            .expect("listener shutdown detaches caller")
                            .expect("ACP read")
                            .is_none(),
                        "MCP transport shutdown must not send session/cancel"
                    );
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
    assert!(tool_names.contains(&"question_list"));
    assert!(tool_names.contains(&"question_answer"));
    assert!(tool_names.contains(&"provider_sessions_list"));
    assert!(tool_names.contains(&"board_thread_subscribe"));
    assert!(tool_names.contains(&"board_thread_unsubscribe"));
    assert!(tool_names.contains(&"board_thread_subscriptions"));
    assert_eq!(tool_names.len(), 107);
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
