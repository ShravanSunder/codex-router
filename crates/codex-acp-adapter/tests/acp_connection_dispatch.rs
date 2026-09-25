use codex_acp_adapter::{
    AcpConnectionInputs, AcpSessionBinding, AcpStoredSessions, HeldBindingCheckout,
    UnmaterializedBindingStore, serve_acp_connection,
};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    io,
    os::unix::fs::PermissionsExt,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[path = "support/conversation_operation_recorder.rs"]
mod conversation_operation_recorder;
use conversation_operation_recorder::AcceptingConversationRecorder;

struct FixtureCatalog;
#[derive(Default)]
struct TestBindingHolder {
    bindings: Mutex<BTreeMap<String, AcpSessionBinding>>,
    held: tokio::sync::Notify,
    create_tasks: tokio_util::task::TaskTracker,
}
impl UnmaterializedBindingStore for TestBindingHolder {
    fn hold(&self, binding: AcpSessionBinding) {
        self.bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(binding.session_id().to_owned(), binding);
        self.held.notify_waiters();
    }
    fn checkout(&self, session_id: &str) -> HeldBindingCheckout {
        self.bindings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(session_id)
            .map_or(HeldBindingCheckout::Missing, |binding| {
                HeldBindingCheckout::Ready(Box::new(binding))
            })
    }
    fn restore(&self, binding: AcpSessionBinding) {
        self.hold(binding);
    }
    fn finish(&self, _session_id: &str) {}
    fn create_tasks(&self) -> tokio_util::task::TaskTracker {
        self.create_tasks.clone()
    }
}
struct DelayedCatalog {
    entered: tokio_util::sync::CancellationToken,
    release: tokio_util::sync::CancellationToken,
}
impl AcpStoredSessions for DelayedCatalog {
    fn list(&self, _params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async {
            self.entered.cancel();
            self.release.cancelled().await;
            Ok(json!({"sessions":[]}))
        })
    }
}

const TEST_SCRATCH: &str =
    "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000099";
fn ensure_test_scratch() {
    use std::os::unix::fs::PermissionsExt;
    assert!(std::fs::create_dir_all(TEST_SCRATCH).is_ok());
    assert!(std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).is_ok());
}

#[tokio::test]
async fn pending_catalog_does_not_block_connection_routing_or_shutdown() {
    ensure_test_scratch();
    // Arrange: the catalog cannot complete until explicitly released.
    let entered = tokio_util::sync::CancellationToken::new();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let task = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: "/unused-native-fixture".into(),
            generation: serde_json::from_value(
                json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
            )
            .unwrap(),
            schemas: fixture_schemas().unwrap(),
            stored_sessions: Arc::new(DelayedCatalog {
                entered: entered.clone(),
                release: tokio_util::sync::CancellationToken::new(),
            }),
            approval_broker: std::sync::Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder: Arc::new(TestBindingHolder::default()),
            recorder: Arc::new(AcceptingConversationRecorder),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await.unwrap();
    let mut response = String::new();
    read.read_line(&mut response).await.unwrap();

    // Act: route another request while the inventory provider is still pending.
    write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"session/list\",\"params\":{}}\n")
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), entered.cancelled())
        .await
        .unwrap();
    write
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"unsupported/request\",\"params\":{}}\n",
        )
        .await
        .unwrap();
    response.clear();
    let routed = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read.read_line(&mut response),
    )
    .await;
    write.shutdown().await.unwrap();
    let shutdown = tokio::time::timeout(std::time::Duration::from_secs(1), task).await;

    // Assert: no head-of-line blocking, including frontend closure.
    assert!(
        routed.is_ok(),
        "catalog lookup blocked unrelated request routing"
    );
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["id"], 3);
    assert_eq!(response["error"]["code"], -32601);
    assert!(shutdown.unwrap().unwrap().is_ok());
}
impl AcpStoredSessions for FixtureCatalog {
    fn list(&self, _params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async {
            Ok(
                json!({"sessions":[{"sessionId":"stored-thread","cwd":"/work","title":"Stored fixture"}]}),
            )
        })
    }
}
#[tokio::test]
async fn public_connection_routes_discovery_and_receipt_guarded_loads_to_native_fixture() {
    ensure_test_scratch();
    use futures_util::{SinkExt, StreamExt};
    let socket = std::path::PathBuf::from(format!("/tmp/acp-dispatch-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket)
        .unwrap_or_else(|error| panic!("backend bind: {error}"));
    let (setup_entered, mut setup_observed) = tokio::sync::mpsc::channel(2);
    let setup_release = Arc::new(tokio::sync::Semaphore::new(0));
    let backend_release = Arc::clone(&setup_release);
    let backend = tokio::spawn(async move {
        let (stream, _) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("backend accept: {error}"));
        let mut wire = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|error| panic!("upgrade: {error}"));
        for method in ["initialize", "initialized", "thread/start", "thread/resume"] {
            let frame = wire
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|error| panic!("frame: {error}"));
            let request: Value = serde_json::from_str(
                frame
                    .to_text()
                    .unwrap_or_else(|error| panic!("text: {error}")),
            )
            .unwrap_or_else(|error| panic!("native request: {error}"));
            assert_eq!(request["method"], method);
            if method == "initialized" {
                continue;
            }
            if method == "thread/start" {
                assert_eq!(request["params"]["model"], "gpt-5.6-sol");
                assert_eq!(
                    request["params"]["config"]["model_reasoning_effort"],
                    "medium"
                );
                assert!(request["params"].get("approvalPolicy").is_none());
                assert!(request["params"].get("approvalsReviewer").is_none());
                assert_eq!(
                    request["params"]["config"]["mcp_servers"]["notes"]["command"],
                    "/usr/bin/example"
                );
            }
            if method == "thread/resume" {
                assert_eq!(request["params"]["threadId"], "created-thread");
                assert_eq!(request["params"]["permissions"], "router-workspace-write");
                assert_eq!(
                    request["params"]["config"]["default_permissions"],
                    "router-workspace-write"
                );
                assert_eq!(
                    request["params"]["config"]["permissions.router-workspace-write.extends"],
                    ":workspace"
                );
                assert_eq!(
                    request["params"]["config"]["permissions.router-workspace-write.filesystem"]
                        [TEST_SCRATCH],
                    "write"
                );
                assert!(
                    request["params"]["config"]
                        .get("permissions.router-workspace-write")
                        .is_none()
                );
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":{"type":"workspaceWrite","writableRoots":[TEST_SCRATCH]},"thread":{"id":"created-thread","cwd":"/work","turns":[]}})
            };
            if method != "initialize" {
                setup_entered.send(method).await.unwrap();
                backend_release.acquire().await.unwrap().forget();
            }
            wire.send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("native reply: {error}"));
        }
    });
    let schemas = fixture_schemas().unwrap();
    let generation = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )
    .unwrap_or_else(|error| panic!("generation: {error}"));
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let task = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: socket.clone(),
            generation,
            schemas,
            stored_sessions: Arc::new(FixtureCatalog),
            approval_broker: std::sync::Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder: Arc::new(TestBindingHolder::default()),
            recorder: Arc::new(AcceptingConversationRecorder),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    for (id, method, params, expected) in [
        (json!(0), "session/list", json!({}), "error"),
        (
            json!(9223372036854775807_i64),
            "initialize",
            json!({"protocolVersion":1}),
            "result",
        ),
        (json!("list"), "session/list", json!({}), "result"),
        (
            json!("new"),
            "session/new",
            json!({"cwd":"/work","mcpServers":[{"name":"notes","command":"/usr/bin/example","args":[],"env":[]}],"_meta":{"codexRouter":{"operationId":collaboration_protocol::OperationId::generate(),"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write","scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}}),
            "result",
        ),
        (
            json!("mismatch"),
            "session/load",
            json!({"sessionId":"created-thread","cwd":"/work","mcpServers":[{"name":"other","command":"/usr/bin/example","args":[],"env":[]}]}),
            "error",
        ),
        (
            json!("load"),
            "session/load",
            json!({"sessionId":"created-thread","cwd":"/work","mcpServers":[{"name":"notes","command":"/usr/bin/example","args":[],"env":[]}]}),
            "result",
        ),
    ] {
        let frame = format!(
            "{}\n",
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
        );
        write
            .write_all(frame.as_bytes())
            .await
            .unwrap_or_else(|error| panic!("write: {error}"));
        if matches!(method, "session/new" | "session/load") && expected == "result" {
            // A native setup is pending: unrelated discovery must still be routed.
            tokio::time::timeout(std::time::Duration::from_secs(3), setup_observed.recv())
                .await
                .unwrap()
                .unwrap();
            if method == "session/load" {
                for (conflict_id, conflict_method, conflict_params) in [
                    ("duplicate-load", "session/load", params.clone()),
                    (
                        "prompt-during-load",
                        "session/prompt",
                        json!({"sessionId":"created-thread","prompt":[{"type":"text","text":"must not dispatch"}]}),
                    ),
                ] {
                    write.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":conflict_id,"method":conflict_method,"params":conflict_params})).as_bytes()).await.unwrap();
                    let mut conflict_response = String::new();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(1),
                        read.read_line(&mut conflict_response),
                    )
                    .await
                    .unwrap()
                    .unwrap();
                    let conflict_response: Value =
                        serde_json::from_str(&conflict_response).unwrap();
                    assert_eq!(conflict_response["id"], conflict_id);
                    assert_eq!(conflict_response["error"]["code"], -32600);
                }
            }
            let concurrent_id = format!("during-{method}");
            write.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":concurrent_id,"method":"session/list","params":{}})).as_bytes()).await.unwrap();
            let mut concurrent_response = String::new();
            let routed = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                read.read_line(&mut concurrent_response),
            )
            .await;
            setup_release.add_permits(1);
            assert!(routed.is_ok(), "native setup blocked unrelated discovery");
            let concurrent_response: Value = serde_json::from_str(&concurrent_response).unwrap();
            assert_eq!(concurrent_response["id"], concurrent_id);
            assert!(concurrent_response.get("result").is_some());
        }
        let mut response = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            read.read_line(&mut response),
        )
        .await
        .unwrap_or_else(|error| panic!("deadline: {error}"))
        .unwrap_or_else(|error| panic!("read: {error}"));
        let response: Value =
            serde_json::from_str(&response).unwrap_or_else(|error| panic!("response: {error}"));
        assert_eq!(response["id"], id);
        assert!(response.get(expected).is_some(), "{response}");
        if id == json!("mismatch") {
            assert_eq!(response["error"]["code"], -32602);
        }
        if method == "session/list" && id == json!("list") {
            assert_eq!(
                response["result"]["sessions"][0]["sessionId"],
                "stored-thread"
            );
        }
    }
    write
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
    task.await
        .unwrap_or_else(|error| panic!("join: {error}"))
        .unwrap_or_else(|error| panic!("serve: {error}"));
    backend
        .await
        .unwrap_or_else(|error| panic!("backend: {error}"));
    std::fs::remove_file(socket).unwrap_or_else(|error| panic!("socket cleanup: {error}"));
}

fn fixture_schemas() -> Result<Arc<NativePayloadSchemas>, Box<dyn std::error::Error>> {
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
        serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))?,
    )]))?;
    Ok(Arc::new(NativePayloadSchemas::from_bundle(&bundle)?))
}

#[tokio::test]
async fn detached_create_finishes_and_holds_its_empty_thread()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::{SinkExt, StreamExt};
    let root = std::env::current_dir()?;
    let scratch = root.join("tmp/scratch/session-00000000-0000-4000-8000-000000000099");
    std::fs::create_dir_all(&scratch)?;
    std::fs::set_permissions(&scratch, std::fs::Permissions::from_mode(0o700))?;
    let socket = std::path::PathBuf::from(format!("tmp/d{:04x}.sock", std::process::id() & 0xffff));
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let native_scratch = scratch.clone();
    let backend = tokio::spawn(async move {
        let mut entered_tx = Some(entered_tx);
        let mut release_rx = Some(release_rx);
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for expected in ["initialize", "initialized", "thread/start"] {
            let frame = wire.next().await.ok_or("native request missing")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request["method"] != expected {
                return Err(format!("expected {expected}, got {}", request["method"]).into());
            }
            if expected == "initialized" {
                continue;
            }
            if expected == "thread/start" {
                if let Some(entered_tx) = entered_tx.take() {
                    let _sent = entered_tx.send(());
                }
                if let Some(release_rx) = release_rx.take() {
                    let _released = release_rx.await;
                }
            }
            let result = if expected == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":{"type":"workspaceWrite","writableRoots":[native_scratch]},
                    "thread":{"id":"detached-thread","cwd":"/work","turns":[]}})
            };
            let _sent = wire
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"id":request["id"],"result":result})
                        .to_string()
                        .into(),
                ))
                .await;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
    let holder = Arc::new(TestBindingHolder::default());
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: socket.clone(),
            generation: serde_json::from_value(
                json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
            )?,
            schemas: fixture_schemas().map_err(|error| format!("{error}"))?,
            stored_sessions: Arc::new(FixtureCatalog),
            approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
            holder: Arc::clone(&holder) as Arc<dyn UnmaterializedBindingStore>,
            recorder: Arc::new(AcceptingConversationRecorder),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n").await?;
    let mut line = String::new();
    read.read_line(&mut line).await?;
    let operation_id = collaboration_protocol::OperationId::generate();
    write.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{
        "cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{
            "operationId":operation_id,"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write",
            "scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":scratch,
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
            "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }}
    }})).as_bytes()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(3), entered_rx)
        .await
        .map_err(|_| "native thread/start was not reached")??;
    drop(write);
    drop(read);
    serving.await??;
    let _released = release_tx.send(());
    let held = async {
        loop {
            let mut changed = Box::pin(holder.held.notified());
            changed.as_mut().enable();
            if holder
                .bindings
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("detached-thread")
            {
                break;
            }
            changed.await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(3), held)
        .await
        .map_err(|_| "detached create did not retain its empty thread")?;
    backend.await??;
    std::fs::remove_file(socket)?;
    Ok(())
}

#[tokio::test]
async fn closed_create_connection_loads_held_binding_without_native_resume_and_prompts()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use futures_util::{SinkExt, StreamExt};
    ensure_test_scratch();
    let socket = std::env::temp_dir().join(format!("held-acp-{}.sock", std::process::id()));
    let listener = tokio::net::UnixListener::bind(&socket)?;
    let backend = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut wire = tokio_tungstenite::accept_async(stream).await?;
        for expected in [
            "initialize",
            "initialized",
            "thread/start",
            "turn/start",
            "thread/read",
        ] {
            let frame = tokio::time::timeout(std::time::Duration::from_secs(3), wire.next())
                .await?
                .ok_or("native connection closed")??;
            let request: Value = serde_json::from_str(frame.to_text()?)?;
            if request["method"] != expected {
                return Err(format!("expected {expected}, got {}", request["method"]).into());
            }
            if expected == "initialized" {
                continue;
            }
            let result = match expected {
                "initialize" => json!({}),
                "thread/start" => json!({
                    "cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request",
                    "approvalsReviewer":"auto_review",
                    "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                    "sandbox":{"type":"workspaceWrite","writableRoots":[TEST_SCRATCH]},
                    "thread":{"id":"created-thread","cwd":"/work","turns":[]}
                }),
                "turn/start" => json!({"turn":{"id":"turn-one"}}),
                _ => {
                    json!({"thread":{"id":"created-thread","model":"gpt-5.6-sol","reasoningEffort":"medium","status":{"type":"idle"},"createdAt":1}})
                }
            };
            wire.send(tokio_tungstenite::tungstenite::Message::Text(
                json!({"id":request["id"],"result":result})
                    .to_string()
                    .into(),
            ))
            .await?;
            if expected == "turn/start" {
                wire.send(tokio_tungstenite::tungstenite::Message::Text(
                    json!({"method":"turn/completed","params":{"threadId":"created-thread","turn":{"id":"turn-one","status":"completed"}}}).to_string().into(),
                )).await?;
            }
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    });
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
        serde_json::to_vec(
            &json!({"definitions":{"v2":definitions,"ServerRequest":{"type":"object"},"ServerNotification":{"type":"object"}}}),
        )?,
    )]))?;
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle)?);
    let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(json!({
        "serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1
    }))?;
    let holder = Arc::new(TestBindingHolder::default());
    let new_connection = |holder: Arc<TestBindingHolder>| AcpConnectionInputs {
        backend_path: socket.clone(),
        generation: generation.clone(),
        schemas: Arc::clone(&schemas),
        stored_sessions: Arc::new(FixtureCatalog),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        holder,
        recorder: Arc::new(AcceptingConversationRecorder),
        retired: tokio_util::sync::CancellationToken::new(),
    };
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_acp_connection(
        server,
        new_connection(Arc::clone(&holder)),
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    async fn call(
        read: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
        write: &mut tokio::net::unix::OwnedWriteHalf,
        id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
                )
                .as_bytes(),
            )
            .await?;
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), read.read_line(&mut line))
            .await??;
        let response: Value = serde_json::from_str(&line)?;
        if response["id"] != id {
            return Err(format!("unexpected response: {response}").into());
        }
        Ok(response)
    }
    call(
        &mut read,
        &mut write,
        "init-1",
        "initialize",
        json!({"protocolVersion":1}),
    )
    .await?;
    let created = call(&mut read, &mut write, "new", "session/new", json!({
        "cwd":"/work","mcpServers":[],
        "_meta":{"codexRouter":{"operationId":collaboration_protocol::OperationId::generate(),"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write",
            "scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,
            "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
            "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}
        }}
    })).await?;
    if created["result"]["sessionId"] != "created-thread" {
        return Err(format!("create failed: {created}").into());
    }
    write.shutdown().await?;
    serving.await??;
    if !holder
        .bindings
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains_key("created-thread")
    {
        return Err("closed ACP connection did not hold its empty thread".into());
    }
    let (client, server) = tokio::net::UnixStream::pair()?;
    let serving = tokio::spawn(serve_acp_connection(
        server,
        new_connection(Arc::clone(&holder)),
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    call(
        &mut read,
        &mut write,
        "init-2",
        "initialize",
        json!({"protocolVersion":1}),
    )
    .await?;
    let mismatch = call(
        &mut read,
        &mut write,
        "wrong-cwd",
        "session/load",
        json!({"sessionId":"created-thread","cwd":"/wrong-workspace","mcpServers":[]}),
    )
    .await?;
    if mismatch.get("error").is_none() {
        return Err("mismatched load adopted the held binding".into());
    }
    let loaded = call(
        &mut read,
        &mut write,
        "load",
        "session/load",
        json!({"sessionId":"created-thread","cwd":"/work","mcpServers":[]}),
    )
    .await?;
    if loaded.get("result").is_none() {
        return Err(format!("held load failed: {loaded}").into());
    }
    let prompted = call(
        &mut read,
        &mut write,
        "prompt",
        "session/prompt",
        json!({
            "sessionId":"created-thread","prompt":[{"type":"text","text":"hello"}]
        }),
    )
    .await?;
    if prompted.get("result").is_none() {
        return Err(format!("held prompt failed: {prompted}").into());
    }
    write.shutdown().await?;
    serving.await??;
    backend.await??;
    std::fs::remove_file(socket)?;
    Ok(())
}
