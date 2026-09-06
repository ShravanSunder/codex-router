use codex_acp_adapter::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io, pin::Pin, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

struct FixtureCatalog;
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

#[tokio::test]
async fn pending_catalog_does_not_block_connection_routing_or_shutdown() {
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
                assert_eq!(
                    request["params"]["config"]["mcp_servers"]["notes"]["command"],
                    "/usr/bin/example"
                );
            }
            if method == "thread/resume" {
                assert_eq!(request["params"], json!({"threadId":"created-thread"}));
            }
            let result = if method == "initialize" {
                json!({})
            } else {
                json!({"cwd":"/work","thread":{"id":"created-thread","cwd":"/work","turns":[]}})
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
            json!({"cwd":"/work","mcpServers":[{"name":"notes","command":"/usr/bin/example","args":[],"env":[]}]}),
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
