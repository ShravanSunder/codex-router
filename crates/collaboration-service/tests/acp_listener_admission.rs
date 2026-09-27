use codex_acp_adapter::NativeStoredSessions;
use collaboration_service::{
    AcpChannelListener, HubEvent, NativeGenerationGate, SessionCommandPort, SessionEventHub,
    UnmaterializedThreadHolder,
};
use message_board::Identity;
use session_event_model::{
    SessionEvent, SessionItem, SessionItemKind, SessionState, ToolCallStatus,
};
use std::{
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    sync::Arc,
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;
#[path = "../../codex-acp-adapter/tests/support/conversation_operation_recorder.rs"]
mod conversation_operation_recorder;
use conversation_operation_recorder::AcceptingConversationRecorder;

#[path = "support/acp_provider_backend.rs"]
mod acp_provider_backend;
use acp_provider_backend::ScriptedProviderBackend;

#[tokio::test]
async fn acp_listener_reports_unavailable_codex_without_closing_the_connection() {
    // Arrange: real private listener with no accepting backend generation.
    let root = std::path::PathBuf::from(format!("/tmp/acp-admission-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let path = root.join("codex-acp.sock");
    let provider = Arc::new(ScriptedProviderBackend::new().unwrap());
    let gate = NativeGenerationGate::default();
    let listener = AcpChannelListener::bind(
        &path,
        gate.clone(),
        Arc::new(NativeStoredSessions::new(root.clone(), "fixture".into())),
        Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        Arc::new(UnmaterializedThreadHolder::new()),
        Arc::new(AcceptingConversationRecorder),
    )
    .unwrap()
    .with_provider_session_backend(
        provider.endpoint.clone(),
        Arc::clone(&provider) as Arc<dyn SessionCommandPort>,
        Arc::clone(&provider) as Arc<dyn SessionEventHub>,
    );
    let stop = CancellationToken::new();
    let task = tokio::spawn(listener.run(stop.clone()));
    // Act: initialize remains available without a native Codex generation.
    let client = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (read, mut write) = client.into_split();
    let mut lines = BufReader::new(read).lines();
    let initialize = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":1,"clientCapabilities":{},"_meta":{
            "router":{"actor":{"kind":"human","humanId":"owner"}},
            "sessionProfile":{"version":1,"elements":["state"]}
        }
    }});
    write
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .unwrap();
    let initialized = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap();
    let initialized: serde_json::Value =
        serde_json::from_str(&initialized.expect("connection closed before initialize")).unwrap();
    assert_eq!(
        initialized["result"]["_meta"]["sessionProfile"]["version"],
        1
    );
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/new\",\"params\":{\"cwd\":\"/tmp\",\"mcpServers\":[],\"_meta\":{\"router\":{\"endpoint\":\"claude-local\"}}}}\n").await.unwrap();
    let provider_response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let provider_response: serde_json::Value = serde_json::from_str(&provider_response).unwrap();
    assert_eq!(provider_response["id"], 3);
    assert_eq!(provider_response["result"]["sessionId"], "claude-session-1");
    assert_eq!(
        provider_response["result"]["_meta"]["router"]["approver"],
        serde_json::to_value(&provider.approver).unwrap()
    );
    assert_eq!(
        provider_response["result"]["_meta"]["router"]["sessionRef"],
        serde_json::to_value(&provider.session).unwrap()
    );
    assert_eq!(
        provider_response["result"]["_meta"]["sessionProfile"]["capabilities"]["steer"],
        true
    );
    let _sent = provider.events.send(HubEvent {
        sequence: 50,
        event: SessionEvent::ItemStarted {
            item: SessionItem {
                item_id: "external-item".into(),
                kind: SessionItemKind::AgentMessage,
                text: Some("Other front door".into()),
            },
        },
    });
    let observed_elsewhere = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let observed_elsewhere: serde_json::Value = serde_json::from_str(&observed_elsewhere).unwrap();
    assert_eq!(observed_elsewhere["method"], "session/update");
    assert_eq!(
        observed_elsewhere["params"]["update"]["content"]["text"],
        "Other front door"
    );
    let _sent = provider.events.send(HubEvent {
        sequence: 51,
        event: SessionEvent::ItemStarted {
            item: SessionItem {
                item_id: "tool-1".into(),
                kind: SessionItemKind::ToolCall {
                    tool_kind: "execute".into(),
                    status: ToolCallStatus::InProgress,
                },
                text: Some("Run checks".into()),
            },
        },
    });
    let tool_update = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let tool_update: serde_json::Value = serde_json::from_str(&tool_update).unwrap();
    assert_eq!(
        tool_update["params"]["update"]["sessionUpdate"],
        "tool_call"
    );
    assert_eq!(tool_update["params"]["update"]["kind"], "execute");
    assert_eq!(tool_update["params"]["update"]["status"], "in_progress");
    assert_eq!(
        provider.created_by.lock().unwrap().as_slice(),
        &[serde_json::from_value::<Identity>(
            serde_json::json!({"kind":"human","humanId":"owner"})
        )
        .unwrap()]
    );
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"session/list\",\"params\":{\"_meta\":{\"router\":{\"endpoint\":\"claude-local\"}}}}\n").await.unwrap();
    let listed = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let listed: serde_json::Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(
        listed["result"]["sessions"][0]["sessionId"],
        "claude-session-1"
    );
    assert_eq!(
        listed["result"]["sessions"][0]["_meta"]["router"]["sessionRef"],
        serde_json::to_value(&provider.session).unwrap()
    );
    assert_eq!(
        listed["result"]["sessions"][0]["_meta"]["router"]["approver"],
        serde_json::to_value(&provider.approver).unwrap()
    );
    *provider.state.lock().unwrap() = SessionState::Unloaded;
    let wrong_cwd = serde_json::json!({"jsonrpc":"2.0","id":15,"method":"session/load","params":{
        "sessionId":"claude-session-1","cwd":"/other","mcpServers":[],
        "_meta":{"router":{"sessionRef":provider.session}}
    }});
    write
        .write_all(format!("{wrong_cwd}\n").as_bytes())
        .await
        .unwrap();
    let wrong_cwd_response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let wrong_cwd_response: serde_json::Value = serde_json::from_str(&wrong_cwd_response).unwrap();
    assert_eq!(wrong_cwd_response["error"]["code"], -32602);
    assert!(provider.loaded_by.lock().unwrap().is_empty());
    let loaded_request = serde_json::json!({"jsonrpc":"2.0","id":9,"method":"session/load","params":{
        "sessionId":"claude-session-1","cwd":"/tmp","mcpServers":[],
        "_meta":{"router":{"sessionRef":provider.session}}
    }});
    write
        .write_all(format!("{loaded_request}\n").as_bytes())
        .await
        .unwrap();
    let replayed = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let replayed: serde_json::Value = serde_json::from_str(&replayed).unwrap();
    assert_eq!(replayed["method"], "session/update");
    assert_eq!(
        replayed["params"]["update"]["content"]["text"],
        "Prior reply"
    );
    let loaded = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let loaded: serde_json::Value = serde_json::from_str(&loaded).unwrap();
    assert_eq!(
        loaded["result"]["_meta"]["router"]["approver"],
        serde_json::to_value(&provider.approver).unwrap()
    );
    assert_eq!(
        loaded["result"]["_meta"]["sessionProfile"]["historyUnavailable"],
        false
    );
    assert_eq!(provider.loaded_by.lock().unwrap().len(), 1);
    let _sent = provider.events.send(HubEvent {
        sequence: 60,
        event: SessionEvent::ResyncRequired { replay_epoch: 1 },
    });
    let resynced = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resynced: serde_json::Value = serde_json::from_str(&resynced).unwrap();
    assert_eq!(
        resynced["params"]["update"]["content"]["text"],
        "Prior reply"
    );
    *provider.state.lock().unwrap() = SessionState::Unloaded;
    let resumed_request = serde_json::json!({"jsonrpc":"2.0","id":10,"method":"session/resume","params":{
        "sessionId":"claude-session-1","_meta":{"router":{"sessionRef":provider.session}}
    }});
    write
        .write_all(format!("{resumed_request}\n").as_bytes())
        .await
        .unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let resumed: serde_json::Value = serde_json::from_str(&resumed).unwrap();
    assert_eq!(
        resumed["result"]["_meta"]["router"]["approver"],
        serde_json::to_value(&provider.approver).unwrap()
    );
    assert_eq!(
        resumed["result"]["_meta"]["sessionProfile"]["historyUnavailable"],
        true
    );
    assert_eq!(provider.resumed_by.lock().unwrap().len(), 1);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"/tmp\",\"mcpServers\":[]}}\n").await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], 2);
    assert_eq!(response["error"]["code"], -32000);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"claude-session-1\",\"prompt\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n").await.unwrap();
    let running = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let running: serde_json::Value = serde_json::from_str(&running).unwrap();
    assert_eq!(running["method"], "_session/state");
    assert_eq!(running["params"]["state"], "running");
    let update = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let update: serde_json::Value = serde_json::from_str(&update).unwrap();
    assert_eq!(update["method"], "session/update");
    assert_eq!(
        update["params"]["update"]["content"]["text"],
        "Claude reply"
    );
    let idle = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let idle: serde_json::Value = serde_json::from_str(&idle).unwrap();
    assert_eq!(idle["method"], "_session/state");
    assert_eq!(idle["params"]["state"], "idle");
    let completed = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
    assert_eq!(completed["method"], "_session/state");
    assert_eq!(completed["params"]["turn"]["status"], "completed");
    assert_eq!(completed["params"]["turn"]["turnId"], "turn-1");
    let prompted = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let prompted: serde_json::Value = serde_json::from_str(&prompted).unwrap();
    assert_eq!(prompted["id"], 4);
    assert_eq!(prompted["result"]["stopReason"], "end_turn");
    assert_eq!(
        provider.prompted_by.lock().unwrap().as_slice(),
        &[serde_json::from_value::<Identity>(
            serde_json::json!({"kind":"human","humanId":"owner"})
        )
        .unwrap()]
    );
    *provider.state.lock().unwrap() = SessionState::Running;
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":11,\"method\":\"_session/steering\",\"params\":{\"sessionId\":\"claude-session-1\",\"prompt\":[{\"type\":\"text\",\"text\":\"follow up\"}]}}\n").await.unwrap();
    let steered = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let steered: serde_json::Value = serde_json::from_str(&steered).unwrap();
    assert_eq!(steered["result"]["outcome"], "injected");
    assert_eq!(provider.steered_by.lock().unwrap().len(), 1);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":12,\"method\":\"_session/queue/add\",\"params\":{\"sessionId\":\"claude-session-1\",\"prompt\":[{\"type\":\"text\",\"text\":\"later\"}]}}\n").await.unwrap();
    let queued = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let queued: serde_json::Value = serde_json::from_str(&queued).unwrap();
    assert_eq!(queued["result"]["inputId"], "input-2");
    assert_eq!(provider.queued_by.lock().unwrap().len(), 1);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":13,\"method\":\"_session/queue/list\",\"params\":{\"sessionId\":\"claude-session-1\"}}\n").await.unwrap();
    let queue_list = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let queue_list: serde_json::Value = serde_json::from_str(&queue_list).unwrap();
    assert_eq!(queue_list["result"]["items"][0]["inputId"], "input-2");
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":14,\"method\":\"_session/queue/cancel\",\"params\":{\"sessionId\":\"claude-session-1\",\"inputId\":\"input-2\"}}\n").await.unwrap();
    let cancelled = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let cancelled: serde_json::Value = serde_json::from_str(&cancelled).unwrap();
    assert_eq!(cancelled["result"]["cancelled"], true);
    *provider.state.lock().unwrap() = SessionState::Idle;
    // A native generation can be admitted later on this same ACP connection.
    use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
    use futures_util::{SinkExt, StreamExt};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    let scratch = root.join("scratch/session-00000000-0000-4000-8000-000000000099");
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut definitions = serde_json::Map::new();
    for name in [
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "ThreadLoadedList",
        "TurnStart",
        "TurnSteer",
        "TurnInterrupt",
    ] {
        definitions.insert(format!("{name}Params"), json!({"type":"object"}));
        definitions.insert(format!("{name}Response"), json!({"type":"object"}));
    }
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&json!({"definitions":{"v2":definitions,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}})).unwrap(),
    )])).unwrap();
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap());
    let backend_path = root.join("backend.sock");
    let backend_listener = tokio::net::UnixListener::bind(&backend_path).unwrap();
    let scratch_for_backend = scratch.clone();
    let backend = tokio::spawn(async move {
        let (stream, _) = backend_listener.accept().await.unwrap();
        let mut wire = tokio_tungstenite::accept_async(stream).await.unwrap();
        for method in ["initialize", "initialized", "thread/start"] {
            let frame = wire.next().await.unwrap().unwrap();
            let request: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
            assert_eq!(request["method"], method);
            if method == "initialized" {
                continue;
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/tmp","model":"gpt-5.6-sol","approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":{"type":"workspaceWrite","writableRoots":[scratch_for_backend]},
                    "thread":{"id":"native-thread-1","cwd":"/tmp","turns":[]}})
            };
            wire.send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
        }
    });
    gate.activate(
        serde_json::from_value(
            json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
        )
        .unwrap(),
        backend_path.clone(),
        Some(schemas),
    )
    .unwrap();
    let operation_id = collaboration_protocol::OperationId::generate();
    let create_native = json!({"jsonrpc":"2.0","id":5,"method":"session/new","params":{
        "cwd":"/tmp","mcpServers":[],"_meta":{"codexRouter":{
            "operationId":operation_id,"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write",
            "scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":scratch,
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
            "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }}
    }});
    write
        .write_all(format!("{create_native}\n").as_bytes())
        .await
        .unwrap();
    let native_response = tokio::time::timeout(Duration::from_secs(3), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let native_response: Value = serde_json::from_str(&native_response).unwrap();
    assert_eq!(native_response["id"], 5);
    assert_eq!(
        native_response["result"]["sessionId"], "native-thread-1",
        "{native_response}"
    );
    gate.retire().unwrap();
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":6,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"native-thread-1\",\"prompt\":[{\"type\":\"text\",\"text\":\"stale\"}]}}\n").await.unwrap();
    let stale_response = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let stale_response: Value = serde_json::from_str(&stale_response).unwrap();
    assert_eq!(stale_response["error"]["code"], -32000);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"session/list\",\"params\":{\"_meta\":{\"router\":{\"endpoint\":\"claude-local\"}}}}\n").await.unwrap();
    let provider_state = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let provider_state: Value = serde_json::from_str(&provider_state).unwrap();
    assert_eq!(
        provider_state["result"]["sessions"][0]["sessionId"],
        "claude-session-1"
    );
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":16,\"method\":\"_session/state\",\"params\":{\"sessionId\":\"claude-session-1\"}}\n").await.unwrap();
    let state_request = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let state_request: Value = serde_json::from_str(&state_request).unwrap();
    assert_eq!(state_request["error"]["code"], -32601);
    let anonymous = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (anonymous_read, mut anonymous_write) = anonymous.into_split();
    let mut anonymous_lines = BufReader::new(anonymous_read).lines();
    anonymous_write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await.unwrap();
    let _initialized = tokio::time::timeout(Duration::from_secs(2), anonymous_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    anonymous_write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/new\",\"params\":{\"cwd\":\"/tmp\",\"mcpServers\":[],\"_meta\":{\"router\":{\"endpoint\":\"claude-local\"}}}}\n").await.unwrap();
    let anonymous_response =
        tokio::time::timeout(Duration::from_secs(2), anonymous_lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    let anonymous_response: Value = serde_json::from_str(&anonymous_response).unwrap();
    assert_eq!(anonymous_response["error"]["code"], -32602);
    anonymous_write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/list\",\"params\":{\"_meta\":{\"router\":{\"endpoint\":\"claude-local\"}}}}\n").await.unwrap();
    let anonymous_list = tokio::time::timeout(Duration::from_secs(2), anonymous_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let anonymous_list: Value = serde_json::from_str(&anonymous_list).unwrap();
    assert_eq!(
        anonymous_list["result"]["sessions"][0]["sessionId"],
        "claude-session-1"
    );
    let plain = tokio::net::UnixStream::connect(&path).await.unwrap();
    let (plain_read, mut plain_write) = plain.into_split();
    let mut plain_lines = BufReader::new(plain_read).lines();
    let plain_initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":1,"_meta":{"router":{"actor":{"kind":"human","humanId":"owner"}}}
    }});
    plain_write
        .write_all(format!("{plain_initialize}\n").as_bytes())
        .await
        .unwrap();
    let _plain_initialized = tokio::time::timeout(Duration::from_secs(2), plain_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let plain_load = json!({"jsonrpc":"2.0","id":2,"method":"session/load","params":{
        "sessionId":"claude-session-1","cwd":"/tmp","mcpServers":[],
        "_meta":{"router":{"sessionRef":provider.session}}
    }});
    plain_write
        .write_all(format!("{plain_load}\n").as_bytes())
        .await
        .unwrap();
    let plain_replay = tokio::time::timeout(Duration::from_secs(2), plain_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let plain_replay: Value = serde_json::from_str(&plain_replay).unwrap();
    assert_eq!(
        plain_replay["params"]["update"]["content"]["text"],
        "Prior reply"
    );
    let plain_loaded = tokio::time::timeout(Duration::from_secs(2), plain_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let plain_loaded: Value = serde_json::from_str(&plain_loaded).unwrap();
    assert_eq!(plain_loaded["id"], 2);
    plain_write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"session/prompt\",\"params\":{\"sessionId\":\"claude-session-1\",\"prompt\":[{\"type\":\"text\",\"text\":\"again\"}]}}\n").await.unwrap();
    let plain_update = tokio::time::timeout(Duration::from_secs(2), plain_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let plain_update: Value = serde_json::from_str(&plain_update).unwrap();
    assert_eq!(plain_update["method"], "session/update");
    let plain_result = tokio::time::timeout(Duration::from_secs(2), plain_lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let plain_result: Value = serde_json::from_str(&plain_result).unwrap();
    assert_eq!(plain_result["id"], 3);
    assert_eq!(plain_result["result"]["stopReason"], "end_turn");
    stop.cancel();
    task.await.unwrap().unwrap();
    backend.await.unwrap();
    assert!(!path.exists());
    std::fs::remove_file(backend_path).unwrap();
    std::fs::remove_dir(scratch).unwrap();
    std::fs::remove_dir(root.join("scratch")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
