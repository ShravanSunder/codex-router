#![allow(clippy::unwrap_used)]

use codex_acp_adapter::{
    AcpConnectionInputs, AcpSchemaCatalog, AcpSessionBinding, AcpStoredSessions,
    HeldBindingCheckout, PromptTaskInputs, SessionSetupInputs, UnmaterializedBindingStore,
    run_prompt_task, serve_acp_connection,
};
use codex_native_integration::{
    NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    os::unix::fs::PermissionsExt,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    sync::oneshot,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

#[path = "support/native_permission_echo.rs"]
mod native_permission_echo;
use native_permission_echo::applied_router_sandbox;
#[path = "support/conversation_operation_recorder.rs"]
mod conversation_operation_recorder;
use conversation_operation_recorder::AcceptingConversationRecorder;

const TEST_SCRATCH: &str =
    "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000099";

#[derive(Default)]
struct TestBindingHolder {
    bindings: Mutex<BTreeMap<String, AcpSessionBinding>>,
    host_tasks: tokio_util::task::TaskTracker,
}
impl UnmaterializedBindingStore for TestBindingHolder {
    fn hold(&self, binding: AcpSessionBinding) {
        self.bindings
            .lock()
            .unwrap()
            .insert(binding.session_id().to_owned(), binding);
    }
    fn checkout(&self, session_id: &str) -> HeldBindingCheckout {
        self.bindings
            .lock()
            .unwrap()
            .remove(session_id)
            .map_or(HeldBindingCheckout::Missing, |binding| {
                HeldBindingCheckout::Ready(Box::new(binding))
            })
    }
    fn restore(&self, binding: AcpSessionBinding) {
        self.hold(binding);
    }
    fn finish(&self, _session_id: &str) {}
    fn host_tasks(&self) -> tokio_util::task::TaskTracker {
        self.host_tasks.clone()
    }
}
struct EmptyCatalog;
impl AcpStoredSessions for EmptyCatalog {
    fn list(&self, _params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async { Ok(json!({"sessions":[]})) })
    }
}

fn fixture_schemas() -> Arc<NativePayloadSchemas> {
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
    Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap())
}

async fn read_native_request(wire: &mut WebSocketStream<tokio::net::UnixStream>) -> Value {
    let frame = tokio::time::timeout(std::time::Duration::from_secs(3), wire.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(frame.to_text().unwrap()).unwrap()
}

async fn reply_native(
    wire: &mut WebSocketStream<tokio::net::UnixStream>,
    request: &Value,
    result: Value,
) {
    wire.send(Message::Text(
        json!({"id":request["id"],"result":result})
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn frontend_eof_detaches_active_turn_and_reconnect_is_busy_until_native_idle() {
    std::fs::create_dir_all(TEST_SCRATCH).unwrap();
    std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = std::env::temp_dir().join(format!("prompt-detach-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (turn_started_tx, turn_started_rx) = oneshot::channel();
    let (publish_update_tx, publish_update_rx) = oneshot::channel();
    let (finish_turn_tx, finish_turn_rx) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let (first_stream, _) = listener.accept().await?;
        let mut first = tokio::spawn(async move {
            let mut wire = tokio_tungstenite::accept_async(first_stream).await?;
            for method in [
                "initialize",
                "initialized",
                "thread/start",
                "thread/read",
                "turn/start",
            ] {
                let request = read_native_request(&mut wire).await;
                assert_eq!(request["method"], method);
                if method == "initialized" {
                    continue;
                }
                let result = match method {
                    "initialize" => json!({}),
                    "thread/start" => {
                        json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":applied_router_sandbox(&request),"thread":{"id":"thread-a","cwd":"/work","status":{"type":"idle"},"turns":[]}})
                    }
                    "thread/read" => {
                        json!({"thread":{"id":"thread-a","status":{"type":"idle"},"turns":[]}})
                    }
                    _ => json!({"turn":{"id":"turn-a","status":"inProgress"}}),
                };
                reply_native(&mut wire, &request, result).await;
                if method == "turn/start" {
                    let _sent = turn_started_tx.send(());
                    publish_update_rx.await.unwrap();
                    wire.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"message-a","delta":"detached output"}}).to_string().into())).await?;
                    wire.send(Message::Text(json!({"id":901,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"tool-a","command":"cargo test","availableDecisions":["accept","decline"]}}).to_string().into())).await?;
                    let approval = read_native_request(&mut wire).await;
                    assert_eq!(approval, json!({"id":901,"result":{"decision":"cancel"}}));
                    tokio::select! {
                        _ = finish_turn_rx => {},
                        message = wire.next() => panic!("frontend detach sent native request before completion: {message:?}"),
                    }
                    wire.send(Message::Text(json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"completed"}}}).to_string().into())).await?;
                    let read = read_native_request(&mut wire).await;
                    assert_eq!(read["method"], "thread/read");
                    reply_native(&mut wire, &read, json!({"thread":{"id":"thread-a","model":"gpt-5.6-sol","reasoningEffort":"medium","createdAt":1,"updatedAt":1}})).await;
                    break;
                }
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });

        let (second_stream, _) = listener.accept().await?;
        let mut second = tokio_tungstenite::accept_async(second_stream).await?;
        for method in ["initialize", "initialized", "thread/resume"] {
            let request = read_native_request(&mut second).await;
            assert_eq!(request["method"], method);
            if method == "initialized" {
                continue;
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":applied_router_sandbox(&request),"thread":{"id":"thread-a","cwd":"/work","status":{"type":"active"},"turns":[{"id":"turn-a","status":"inProgress","items":[]}]}})
            };
            reply_native(&mut second, &request, result).await;
        }
        let mut second_open = true;
        while second_open {
            tokio::select! {
                message = second.next() => match message {
                    Some(Ok(frame)) => panic!("busy reconnect dispatched another native request: {frame}"),
                    Some(Err(_)) | None => second_open = false,
                },
                result = &mut first => {
                    result??;
                    second_open = false;
                },
            }
        }
        if !first.is_finished() {
            first.await??;
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let schemas = fixture_schemas();
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let holder = Arc::new(TestBindingHolder::default());
    let connection_inputs = || AcpConnectionInputs {
        backend_path: socket.clone(),
        generation: generation.clone(),
        schemas: Arc::clone(&schemas),
        stored_sessions: Arc::new(EmptyCatalog),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        holder: holder.clone(),
        recorder: Arc::new(AcceptingConversationRecorder),
        retired: tokio_util::sync::CancellationToken::new(),
    };

    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let mut first_serving = tokio::spawn(serve_acp_connection(server, connection_inputs()));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    async fn call(
        read: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
        write: &mut tokio::net::unix::OwnedWriteHalf,
        id: &str,
        method: &str,
        params: Value,
    ) -> Value {
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    assert!(
        call(
            &mut read,
            &mut write,
            "init-1",
            "initialize",
            json!({"protocolVersion":1})
        )
        .await
        .get("result")
        .is_some()
    );
    let created = call(&mut read, &mut write, "new", "session/new", json!({"cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{"operationId":collaboration_protocol::OperationId::generate(),"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write","scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}})).await;
    assert_eq!(created["result"]["sessionId"], "thread-a");
    write.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt","params":{"sessionId":"thread-a","prompt":[{"type":"text","text":"work"}],"_meta":{"codexRouter":{"effort":"medium"}}}})).as_bytes()).await.unwrap();
    turn_started_rx.await.unwrap();
    write.shutdown().await.unwrap();
    drop(write);
    drop(read);
    let _update_authorized = publish_update_tx.send(());
    // The frontend's connection and listener permit must be released while
    // the native turn is still running, not after its terminal arrives.
    tokio::time::timeout(std::time::Duration::from_secs(3), &mut first_serving)
        .await
        .expect("ACP connection should exit at frontend EOF")
        .unwrap()
        .unwrap();

    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let second_serving = tokio::spawn(serve_acp_connection(server, connection_inputs()));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    assert!(
        call(
            &mut read,
            &mut write,
            "init-2",
            "initialize",
            json!({"protocolVersion":1})
        )
        .await
        .get("result")
        .is_some()
    );
    let busy = call(
        &mut read,
        &mut write,
        "load",
        "session/load",
        json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]}),
    )
    .await;
    assert_eq!(busy["error"]["code"], -32600);
    assert_eq!(
        busy["error"]["message"],
        "Native session has an active turn"
    );
    assert_eq!(busy["error"]["data"]["kind"], "busy");
    let refused = call(
        &mut read,
        &mut write,
        "prompt",
        "session/prompt",
        json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"must not dispatch"}]}),
    )
    .await;
    assert_eq!(refused["error"]["code"], -32600);
    write.shutdown().await.unwrap();
    second_serving.await.unwrap().unwrap();

    let _finish_authorized = finish_turn_tx.send(());
    holder.host_tasks.close();
    holder.host_tasks.wait().await;
    backend.await.unwrap().unwrap();
    std::fs::remove_file(socket).unwrap();
}

#[tokio::test]
async fn native_callback_after_prompt_command_eof_interrupts_active_turn() {
    std::fs::create_dir_all(TEST_SCRATCH).unwrap();
    std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let (turn_started_tx, turn_started_rx) = oneshot::channel();
    let (callback_tx, callback_rx) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut wire = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
        let resume = read_native_request(&mut wire).await;
        assert_eq!(resume["method"], "thread/resume");
        reply_native(
            &mut wire,
            &resume,
            json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":"thread-callback-eof","cwd":"/work","status":{"type":"idle"},"turns":[]}}),
        )
        .await;
        let activity = read_native_request(&mut wire).await;
        assert_eq!(activity["method"], "thread/read");
        reply_native(
            &mut wire,
            &activity,
            json!({"thread":{"id":"thread-callback-eof","status":{"type":"idle"},"turns":[]}}),
        )
        .await;
        let start = read_native_request(&mut wire).await;
        assert_eq!(start["method"], "turn/start");
        reply_native(
            &mut wire,
            &start,
            json!({"turn":{"id":"turn-callback-eof","status":"inProgress"}}),
        )
        .await;
        let _sent = turn_started_tx.send(());
        callback_rx.await.unwrap();
        wire.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-callback-eof","turnId":"turn-callback-eof","itemId":"message-eof","delta":"discarded after detach"}}).to_string().into())).await.unwrap();
        wire.send(Message::Text(json!({"id":902,"method":"item/unsupported/request","params":{"threadId":"thread-callback-eof","turnId":"turn-callback-eof"}}).to_string().into())).await.unwrap();
        let interrupt = read_native_request(&mut wire).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(
            interrupt["params"],
            json!({"threadId":"thread-callback-eof","turnId":"turn-callback-eof"})
        );
        reply_native(
            &mut wire,
            &interrupt,
            json!({"turn":{"id":"turn-callback-eof","status":"interrupted"}}),
        )
        .await;
    });

    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let mut catalog = AcpSchemaCatalog::load().unwrap();
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let session = AcpSessionBinding::load_existing(
        &mut catalog,
        SessionSetupInputs {
            operation_id: None,
            recorder: Arc::new(AcceptingConversationRecorder),
            connection,
            schemas: fixture_schemas(),
            generation,
            params: json!({"sessionId":"thread-callback-eof","cwd":"/work","mcpServers":[]}),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        },
    )
    .await
    .map(|(session, _history)| session)
    .unwrap();
    let closed = tokio_util::sync::CancellationToken::new();
    let (output, frames) = codex_acp_adapter::bounded_acp_output(closed.clone());
    drop(frames);
    let (commands, receiver) = tokio::sync::mpsc::channel(1);
    let prompt = tokio::spawn(run_prompt_task(PromptTaskInputs {
        session,
        request_id: json!("prompt-eof"),
        params: json!({"sessionId":"thread-callback-eof","prompt":[{"type":"text","text":"work"}]}),
        commands: receiver,
        output,
        retired: tokio_util::sync::CancellationToken::new(),
    }));
    turn_started_rx.await.unwrap();

    // This is the registry's EOF signal for the prompt actor. Closing the
    // output carrier also ensures any update after detachment cannot reach ACP.
    drop(commands);
    closed.cancel();
    let _callback_sent = callback_tx.send(());
    let completion = tokio::time::timeout(std::time::Duration::from_secs(3), prompt)
        .await
        .unwrap()
        .unwrap();
    assert!(completion.terminal.is_none());
    backend.await.unwrap();
}

#[tokio::test]
async fn native_callback_after_frontend_eof_interrupts_turn_and_finishes_actor() {
    std::fs::create_dir_all(TEST_SCRATCH).unwrap();
    std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket =
        std::env::temp_dir().join(format!("callback-after-eof-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (turn_started_tx, turn_started_rx) = oneshot::channel();
    let (callback_tx, callback_rx) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for method in ["initialize", "initialized", "thread/resume"] {
            let request = read_native_request(&mut wire).await;
            assert_eq!(request["method"], method);
            if method == "initialized" {
                continue;
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":"thread-callback","cwd":"/work","status":{"type":"idle"},"turns":[]}})
            };
            reply_native(&mut wire, &request, result).await;
        }

        let activity = read_native_request(&mut wire).await;
        assert_eq!(activity["method"], "thread/read");
        reply_native(
            &mut wire,
            &activity,
            json!({"thread":{"id":"thread-callback","status":{"type":"idle"},"turns":[]}}),
        )
        .await;
        let start = read_native_request(&mut wire).await;
        assert_eq!(start["method"], "turn/start");
        reply_native(
            &mut wire,
            &start,
            json!({"turn":{"id":"turn-callback","status":"inProgress"}}),
        )
        .await;
        let _sent = turn_started_tx.send(());
        callback_rx.await.unwrap();
        wire.send(Message::Text(json!({"id":901,"method":"item/unknown/request","params":{"threadId":"thread-callback","turnId":"turn-callback"}}).to_string().into())).await?;

        let interrupt = read_native_request(&mut wire).await;
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(
            interrupt["params"],
            json!({"threadId":"thread-callback","turnId":"turn-callback"})
        );
        reply_native(
            &mut wire,
            &interrupt,
            json!({"turn":{"id":"turn-callback","status":"interrupted"}}),
        )
        .await;
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let schemas = fixture_schemas();
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let inputs = AcpConnectionInputs {
        backend_path: socket.clone(),
        generation,
        schemas,
        stored_sessions: Arc::new(EmptyCatalog),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        holder: Arc::new(TestBindingHolder::default()),
        recorder: Arc::new(AcceptingConversationRecorder),
        retired: tokio_util::sync::CancellationToken::new(),
    };
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = tokio::spawn(serve_acp_connection(server, inputs));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    async fn call(
        read: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
        write: &mut tokio::net::unix::OwnedWriteHalf,
        id: &str,
        method: &str,
        params: Value,
    ) -> Value {
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }
    let initialized = call(
        &mut read,
        &mut write,
        "initialize",
        "initialize",
        json!({"protocolVersion":1}),
    )
    .await;
    assert!(initialized.get("result").is_some());
    let loaded = call(
        &mut read,
        &mut write,
        "load",
        "session/load",
        json!({"sessionId":"thread-callback","cwd":"/work","mcpServers":[]}),
    )
    .await;
    assert!(loaded.get("result").is_some(), "load failed: {loaded}");
    write
        .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":"prompt","method":"session/prompt","params":{"sessionId":"thread-callback","prompt":[{"type":"text","text":"work"}]}})).as_bytes())
        .await
        .unwrap();
    turn_started_rx.await.unwrap();
    write.shutdown().await.unwrap();
    drop(write);
    drop(read);
    let _callback_sent = callback_tx.send(());

    serving.await.unwrap().unwrap();
    backend.await.unwrap().unwrap();
    std::fs::remove_file(socket).unwrap();
}

#[tokio::test]
async fn session_load_without_native_thread_status_fails_closed() {
    std::fs::create_dir_all(TEST_SCRATCH).unwrap();
    std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = std::env::temp_dir().join(format!("missing-status-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let (prompt_attempted_tx, prompt_attempted_rx) = oneshot::channel();
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for method in ["initialize", "initialized", "thread/resume"] {
            let request = read_native_request(&mut wire).await;
            assert_eq!(request["method"], method);
            if method == "initialized" {
                continue;
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                // The test schema deliberately accepts this object so the
                // adapter's required-status guard is exercised directly.
                json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":"thread-missing-status","cwd":"/work","turns":[]}})
            };
            reply_native(&mut wire, &request, result).await;
        }
        prompt_attempted_rx.await.unwrap();
        if let Ok(Some(Ok(frame))) =
            tokio::time::timeout(std::time::Duration::from_millis(100), wire.next()).await
        {
            panic!("missing-status load dispatched native work: {frame}");
        }
        Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
    });

    let schemas = fixture_schemas();
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap();
    let holder = Arc::new(TestBindingHolder::default());
    let inputs = AcpConnectionInputs {
        backend_path: socket.clone(),
        generation,
        schemas,
        stored_sessions: Arc::new(EmptyCatalog),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        holder,
        recorder: Arc::new(AcceptingConversationRecorder),
        retired: tokio_util::sync::CancellationToken::new(),
    };
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = tokio::spawn(serve_acp_connection(server, inputs));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    async fn call(
        read: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
        write: &mut tokio::net::unix::OwnedWriteHalf,
        id: &str,
        method: &str,
        params: Value,
    ) -> Value {
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), read.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        serde_json::from_str(&line).unwrap()
    }

    let initialized = call(
        &mut read,
        &mut write,
        "initialize",
        "initialize",
        json!({"protocolVersion":1}),
    )
    .await;
    assert!(initialized.get("result").is_some());
    let loaded = call(
        &mut read,
        &mut write,
        "load",
        "session/load",
        json!({"sessionId":"thread-missing-status","cwd":"/work","mcpServers":[]}),
    )
    .await;
    assert_eq!(loaded["error"]["code"], -32603);
    assert_eq!(loaded["error"]["data"]["kind"], "invalidThreadStatus");
    let prompt = call(
        &mut read,
        &mut write,
        "prompt",
        "session/prompt",
        json!({"sessionId":"thread-missing-status","prompt":[{"type":"text","text":"must not dispatch"}]}),
    )
    .await;
    assert_eq!(prompt["error"]["code"], -32600);
    let _signalled = prompt_attempted_tx.send(());
    write.shutdown().await.unwrap();
    serving.await.unwrap().unwrap();
    backend.await.unwrap().unwrap();
    std::fs::remove_file(socket).unwrap();
}
