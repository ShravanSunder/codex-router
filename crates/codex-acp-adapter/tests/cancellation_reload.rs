use codex_acp_adapter::{AcpConnectionInputs, AcpStoredSessions, serve_acp_connection};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, future::Future, io, pin::Pin, sync::Arc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_tungstenite::tungstenite::Message;

struct EmptyCatalog;
impl AcpStoredSessions for EmptyCatalog {
    fn list(&self, _: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async { Ok(json!({"sessions":[]})) })
    }
}

#[tokio::test]
async fn rejected_interrupt_stays_blocked_through_active_reload_and_clears_after_idle_reload() {
    // Arrange: one native turn rejects interruption; reload first sees it active, then idle.
    let socket = format!("/tmp/acp-cancel-reload-{}.sock", std::process::id());
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let backend = tokio::spawn(async move {
        for phase in 0..3 {
            let (stream, _) = listener.accept().await.unwrap();
            let mut wire = tokio_tungstenite::accept_async(stream).await.unwrap();
            let methods: &[&str] = if phase == 0 {
                &[
                    "initialize",
                    "initialized",
                    "thread/start",
                    "turn/start",
                    "turn/interrupt",
                ]
            } else {
                &["initialize", "initialized", "thread/resume"]
            };
            for method in methods {
                let frame = wire.next().await.unwrap().unwrap();
                let request: Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
                assert_eq!(request["method"], *method);
                if *method == "initialized" {
                    continue;
                }
                let result = match *method {
                    "initialize" => json!({}),
                    "turn/start" => json!({"turn":{"id":"target","status":"inProgress"}}),
                    _ => {
                        json!({"cwd":"/work","thread":{"id":"thread-a","cwd":"/work","status":{"type":if phase==1 {"active"} else {"idle"}},"turns":if phase==1 {json!([{"id":"target","status":"inProgress","items":[]}])} else {json!([])}}})
                    }
                };
                let reply = if *method == "turn/interrupt" {
                    assert_eq!(
                        request["params"],
                        json!({"threadId":"thread-a","turnId":"target"})
                    );
                    json!({"id":request["id"],"error":{"code":-32603,"message":"fixture rejected interruption"}})
                } else {
                    json!({"id":request["id"],"result":result})
                };
                wire.send(Message::Text(reply.to_string().into()))
                    .await
                    .unwrap();
                if *method == "turn/start" {
                    wire.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"target","itemId":"message-a","delta":"accepted"}}).to_string().into())).await.unwrap();
                }
            }
        }
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
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([("codex_app_server_protocol.schemas.json".to_owned(), serde_json::to_vec(&json!({"definitions":{
        "v2":definitions,
        "ServerRequest":{"type":"object","required":["id","method","params"]},
        "ServerNotification":{"type":"object","required":["method","params"],"properties":{"method":{"const":"item/agentMessage/delta"},"params":{"type":"object","required":["threadId","turnId","itemId","delta"]}}}
    }})).unwrap())])).unwrap();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = tokio::spawn(serve_acp_connection(
        server,
        AcpConnectionInputs {
            backend_path: socket.clone().into(),
            generation: serde_json::from_value(
                json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
            )
            .unwrap(),
            schemas: Arc::new(NativePayloadSchemas::from_bundle(&bundle).unwrap()),
            stored_sessions: Arc::new(EmptyCatalog),
            retired: tokio_util::sync::CancellationToken::new(),
        },
    ));
    let (read, mut write) = client.into_split();
    let mut read = BufReader::new(read);
    let scenario = async {
        for (id, method, params) in [
            (1, "initialize", json!({"protocolVersion":1})),
            (2, "session/new", json!({"cwd":"/work","mcpServers":[]})),
        ] {
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
            let mut response = String::new();
            read.read_line(&mut response).await.unwrap();
            assert!(
                serde_json::from_str::<Value>(&response)
                    .unwrap()
                    .get("result")
                    .is_some()
            );
        }
        // Act: observe acceptance before cancelling so the exact interruption target is known.
        let prompt = json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"work"}]});
        write
            .write_all(
                format!(
                    "{}\n",
                    json!({"jsonrpc":"2.0","id":3,"method":"session/prompt","params":prompt})
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        read.read_line(&mut response).await.unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap()["method"],
            "session/update"
        );
        write.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"session/cancel\",\"params\":{\"sessionId\":\"thread-a\"}}\n").await.unwrap();
        response.clear();
        read.read_line(&mut response).await.unwrap();
        let cancelled: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(cancelled["result"]["stopReason"], "cancelled");
        assert_eq!(
            cancelled["result"]["_meta"]["codex-router/nativeInterruption"]["state"],
            "rejected"
        );
        // Assert: active reload and another prompt fail; only idle reload releases the barrier.
        for (id, method, params, expected) in [
            (
                4,
                "session/load",
                json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]}),
                "error",
            ),
            (5, "session/prompt", prompt, "error"),
            (
                6,
                "session/load",
                json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]}),
                "result",
            ),
        ] {
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
            response.clear();
            read.read_line(&mut response).await.unwrap();
            let response: Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["id"], id);
            assert!(response.get(expected).is_some(), "{response}");
        }
        write.shutdown().await.unwrap();
        serving.await.unwrap().unwrap();
        backend.await.unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), scenario)
        .await
        .unwrap();
    std::fs::remove_file(socket).unwrap();
}
