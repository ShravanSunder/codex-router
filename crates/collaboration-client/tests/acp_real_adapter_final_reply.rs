use codex_acp_adapter::{
    AcpConnectionInputs, AcpSchemaCatalog, AcpStoredSessions, HeldBindingCheckout,
    UnmaterializedBindingStore, serve_acp_connection,
};
use codex_native_integration::{NativePayloadSchemas, NativeSchemaBundle};
use collaboration_client::{
    AcpConversation, ConversationPromptRequest, ExistingConversationPromptRequest,
    PublicPromptContent,
};
use collaboration_protocol::{EndpointId, MessageText, SessionRef};
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap, future::Future, io, os::unix::fs::DirBuilderExt, pin::Pin, sync::Arc,
    time::Duration,
};
use tokio::net::UnixStream;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};
use tokio_util::sync::CancellationToken;

struct EmptyStoredSessionCatalog;

impl AcpStoredSessions for EmptyStoredSessionCatalog {
    fn list(&self, _params: Value) -> Pin<Box<dyn Future<Output = io::Result<Value>> + Send + '_>> {
        Box::pin(async { Ok(json!({"sessions":[]})) })
    }
}

#[derive(Default)]
struct EmptyUnmaterializedBindings {
    host_tasks: tokio_util::task::TaskTracker,
}

impl UnmaterializedBindingStore for EmptyUnmaterializedBindings {
    fn hold(&self, _binding: codex_acp_adapter::AcpSessionBinding) {}

    fn checkout(&self, _session_id: &str) -> HeldBindingCheckout {
        HeldBindingCheckout::Missing
    }

    fn restore(&self, _binding: codex_acp_adapter::AcpSessionBinding) {}

    fn finish(&self, _session_id: &str) {}

    fn host_tasks(&self) -> tokio_util::task::TaskTracker {
        self.host_tasks.clone()
    }
}

struct AcceptingConversationRecorder;

impl codex_acp_adapter::ConversationOperationRecorder for AcceptingConversationRecorder {
    fn admit_create<'a>(
        &'a self,
        _operation_id: &'a collaboration_protocol::OperationId,
        _generation: &'a collaboration_protocol::CodexGeneration,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn before_native_dispatch<'a>(
        &'a self,
        _operation_id: &'a collaboration_protocol::OperationId,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn record_created<'a>(
        &'a self,
        _operation_id: &'a collaboration_protocol::OperationId,
        _session_id: &'a collaboration_protocol::SessionId,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn record_failure<'a>(
        &'a self,
        _operation_id: &'a collaboration_protocol::OperationId,
        _known_not_submitted: bool,
    ) -> codex_acp_adapter::ConversationRecordFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn native_payload_schemas() -> Result<Arc<NativePayloadSchemas>, String> {
    let definitions: serde_json::Map<String, Value> = [
        "ThreadFork",
        "ThreadLoadedList",
        "ThreadRead",
        "ThreadResume",
        "ThreadStart",
        "TurnInterrupt",
        "TurnStart",
        "TurnSteer",
    ]
    .into_iter()
    .flat_map(|name| {
        [
            (format!("{name}Params"), json!({"type":"object"})),
            (format!("{name}Response"), json!({"type":"object"})),
        ]
    })
    .collect();
    let schema_bytes = serde_json::to_vec(&json!({"definitions":{
        "v2":definitions,
        "ServerRequest":{"type":"object"},
        "ServerNotification":{"type":"object"}
    }}))
    .map_err(|error| error.to_string())?;
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        schema_bytes,
    )]))
    .map_err(|error| error.to_string())?;
    let schemas = NativePayloadSchemas::from_bundle(&bundle).map_err(|error| error.to_string())?;
    Ok(Arc::new(schemas))
}

async fn next_native_message(socket: &mut WebSocketStream<UnixStream>) -> Result<Value, String> {
    loop {
        let Some(frame) = socket.next().await else {
            return Err("native connection closed".to_owned());
        };
        let frame = frame.map_err(|error| error.to_string())?;
        if let Message::Text(text) = frame {
            return serde_json::from_str(&text).map_err(|error| error.to_string());
        }
    }
}

async fn send_native_result(
    socket: &mut WebSocketStream<UnixStream>,
    request: &Value,
    result: Value,
) -> Result<(), String> {
    socket
        .send(Message::Text(
            json!({"id":request["id"],"result":result})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|error| error.to_string())
}

async fn send_native_notification(
    socket: &mut WebSocketStream<UnixStream>,
    value: Value,
) -> Result<(), String> {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .map_err(|error| error.to_string())
}

#[tokio::test]
async fn aggregate_long_turn_settles_through_real_adapter_after_many_native_updates() {
    let unique_prefix = uuid::Uuid::now_v7()
        .to_string()
        .chars()
        .take(12)
        .collect::<String>();
    let root = std::path::PathBuf::from(format!("/tmp/acp-real-{unique_prefix}"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let service_id = "00000000-0000-4000-8000-000000000021";
    let digest = format!("sha256:{}", "e".repeat(64));
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let identity = ServiceIdentity::new(service_id, service_id)
        .unwrap()
        .with_endpoints(vec![serde_json::from_value(json!({
            "endpoint":endpoint.clone(),
            "label":"Real adapter fixture",
            "availability":{"state":"available","observedAt":"2026-10-02T00:00:00Z"},
            "channels":[{"kind":"acp","transport":"unixJsonLines","path":"acp.sock","schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
        })).unwrap()])
        .unwrap();
    let control = LocalControlService::bind(&root.join("control.sock"), identity).unwrap();
    let manifest: collaboration_protocol::ServiceManifest = serde_json::from_value(json!({
        "version":2,
        "serviceId":service_id,
        "serviceEpoch":service_id,
        "machineLabel":"adapter-fixture",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest,
        "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
    }))
    .unwrap();
    let publication = ManifestPublication::publish(&root, &manifest).unwrap();
    let stop = CancellationToken::new();
    let control_task = tokio::spawn(control.run(stop.clone()));

    let native_cwd = "/work".to_owned();
    let native_listener = tokio::net::UnixListener::bind(root.join("native.sock")).unwrap();
    let mut native_task = tokio::spawn(async move {
        let (stream, _) = native_listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let initialize = next_native_message(&mut socket).await.unwrap();
        assert_eq!(initialize["method"], "initialize");
        send_native_result(&mut socket, &initialize, json!({}))
            .await
            .unwrap();
        let initialized = next_native_message(&mut socket).await.unwrap();
        assert_eq!(initialized["method"], "initialized");

        let resume = next_native_message(&mut socket).await.unwrap();
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "long-thread");
        let historical_items = (0..1025)
            .map(|index| json!({"type":"agentMessage","id":format!("history-{index}"),"text":"history"}))
            .collect::<Vec<_>>();
        send_native_result(
            &mut socket,
            &resume,
            json!({"cwd":native_cwd,"model":"gpt-5.6-sol","thread":{"id":"long-thread","cwd":native_cwd,"status":{"type":"idle"},"reasoningEffort":"low","turns":[{"id":"history-turn","status":"completed","items":historical_items}]}}),
        )
        .await
        .unwrap();

        for (turn_index, turn_id, final_text) in [
            (0, "turn-a", "the selected final reply"),
            (1, "turn-b", "reply after existing-binding resume"),
        ] {
            if turn_index > 0 {
                let resume = next_native_message(&mut socket).await.unwrap();
                assert_eq!(resume["method"], "thread/resume");
                assert_eq!(resume["params"]["threadId"], "long-thread");
                let historical_items = (0..1025)
                    .map(|index| json!({"type":"agentMessage","id":format!("history-again-{index}"),"text":"history"}))
                    .collect::<Vec<_>>();
                send_native_result(
                    &mut socket,
                    &resume,
                    json!({"cwd":native_cwd,"model":"gpt-5.6-sol","thread":{"id":"long-thread","cwd":native_cwd,"status":{"type":"idle"},"reasoningEffort":"low","turns":[{"id":"history-turn-again","status":"completed","items":historical_items}]}}),
                )
                .await
                .unwrap();
            }

            let thread_read = next_native_message(&mut socket).await.unwrap();
            assert_eq!(thread_read["method"], "thread/read");
            send_native_result(
                &mut socket,
                &thread_read,
                json!({"thread":{"id":"long-thread","status":{"type":"idle"},"turns":[]}}),
            )
            .await
            .unwrap();

            let turn_start = next_native_message(&mut socket).await.unwrap();
            assert_eq!(turn_start["method"], "turn/start");
            send_native_result(&mut socket, &turn_start, json!({"turn":{"id":turn_id}}))
                .await
                .unwrap();

            if turn_index == 0 {
                for index in 0..1025 {
                    send_native_notification(
                        &mut socket,
                        json!({"method":"item/agentMessage/delta","params":{"threadId":"long-thread","turnId":turn_id,"itemId":"message-a","delta":format!("chunk-{index} ")}}),
                    )
                    .await
                    .unwrap();
                }
                send_native_notification(
                    &mut socket,
                    json!({"method":"item/started","params":{"threadId":"long-thread","turnId":turn_id,"item":{"type":"commandExecution","id":"tool-a","status":"inProgress","command":"cargo test"}}}),
                )
                .await
                .unwrap();
                send_native_notification(
                    &mut socket,
                    json!({"method":"item/completed","params":{"threadId":"long-thread","turnId":turn_id,"item":{"type":"commandExecution","id":"tool-a","status":"completed","command":"cargo test","aggregatedOutput":"passed"}}}),
                )
                .await
                .unwrap();
            }
            send_native_notification(
                &mut socket,
                json!({"method":"item/completed","params":{"threadId":"long-thread","turnId":turn_id,"item":{"type":"agentMessage","id":format!("final-{turn_id}"),"phase":"final_answer","text":final_text}}}),
            )
            .await
            .unwrap();
            send_native_notification(
                &mut socket,
                json!({"method":"turn/completed","params":{"threadId":"long-thread","turn":{"id":turn_id,"status":"completed"}}}),
            )
            .await
            .unwrap();

            let receipt_read = next_native_message(&mut socket).await.unwrap();
            assert_eq!(receipt_read["method"], "thread/read");
            send_native_result(
                &mut socket,
                &receipt_read,
                json!({"thread":{"id":"long-thread","model":"gpt-5.6-sol","reasoningEffort":"low","createdAt":1_800_000_000_i64,"updatedAt":1_800_000_000_i64,"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request","approvalsReviewer":"auto_review"}}),
            )
            .await
            .unwrap();
        }
    });

    let acp_listener = tokio::net::UnixListener::bind(root.join("acp.sock")).unwrap();
    let (acp_ready_tx, acp_ready_rx) = tokio::sync::oneshot::channel();
    let adapter_root = root.clone();
    let adapter_service_epoch = service_id.to_owned();
    let acp_task = tokio::spawn(async move {
        let (stream, _) = acp_listener.accept().await.unwrap();
        let _sent = acp_ready_tx.send(());
        serve_acp_connection(
            stream,
            AcpConnectionInputs {
                backend_path: adapter_root.join("native.sock"),
                generation: serde_json::from_value(json!({
                    "serviceEpoch":adapter_service_epoch,
                    "generation":1
                }))
                .unwrap(),
                schemas: native_payload_schemas().unwrap(),
                stored_sessions: Arc::new(EmptyStoredSessionCatalog),
                retired: CancellationToken::new(),
                approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
                holder: Arc::new(EmptyUnmaterializedBindings::default()),
                recorder: Arc::new(AcceptingConversationRecorder),
            },
        )
        .await
        .unwrap();
    });

    let target: SessionRef = serde_json::from_value(json!({
        "endpoint":endpoint,
        "sessionId":"long-thread"
    }))
    .unwrap();
    let mut acp_schemas = AcpSchemaCatalog::load().unwrap();
    let load_params = json!({
        "sessionId":"long-thread",
        "cwd":root,
        "mcpServers":[],
        "_meta":{
            "codexRouter":{},
            "router":{"sessionRef":{"endpoint":{"serviceId":service_id,"endpointId":"codex-local"},"sessionId":"long-thread"}},
            "codex-router/replayHistory":false
        }
    });
    assert!(
        acp_schemas
            .validate("LoadSessionRequest", &load_params)
            .unwrap()
    );
    let endpoint_id = EndpointId::try_from("codex-local".to_owned()).unwrap();
    let mut conversation = AcpConversation::connect(&root, endpoint_id).await.unwrap();
    let prompt_request = |target: SessionRef, text: &str| ExistingConversationPromptRequest {
        target,
        cwd: std::path::PathBuf::from("/work"),
        prompt: ConversationPromptRequest {
            message: PublicPromptContent::HumanUser {
                text: MessageText::try_from(text.to_owned()).unwrap(),
            },
            effort: None,
            timeout_seconds: 30,
        },
    };
    for (prompt_text, expected_reply) in [
        ("summarize this long turn", "the selected final reply"),
        (
            "continue after existing-binding resume",
            "reply after existing-binding resume",
        ),
    ] {
        let prompt_result = tokio::time::timeout(
            Duration::from_secs(30),
            conversation.prompt_existing_on_connection(
                prompt_request(target.clone(), prompt_text),
                CancellationToken::new(),
            ),
        )
        .await
        .expect("aggregate prompt deadline");
        let prompt = match prompt_result {
            Ok(prompt) => prompt,
            Err(error) => {
                let native_status =
                    tokio::time::timeout(Duration::from_millis(200), &mut native_task).await;
                panic!("aggregate prompt: {error:?}; native fixture status: {native_status:?}");
            }
        };
        let encoded = serde_json::to_value(&prompt).unwrap();
        assert_eq!(encoded["end"], "completed");
        assert_eq!(
            encoded["output"],
            json!({"kind":"available","text":expected_reply})
        );
        assert!(encoded.get("updates").is_none());
        assert_eq!(encoded["result"]["stopReason"], "end_turn");
    }

    let _ = acp_ready_rx.await;
    native_task.await.unwrap();
    drop(conversation);
    tokio::time::timeout(Duration::from_secs(3), acp_task)
        .await
        .expect("adapter shutdown")
        .unwrap();
    stop.cancel();
    control_task.await.unwrap().unwrap();
    drop(publication);
    std::fs::remove_file(root.join("acp.sock")).unwrap();
    std::fs::remove_file(root.join("native.sock")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
