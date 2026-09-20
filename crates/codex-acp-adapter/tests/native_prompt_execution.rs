use codex_acp_adapter::{
    AcpSchemaCatalog, AcpSessionBinding, PendingAcpPrompt, PromptEvent, SessionSetupInputs,
};
use codex_native_integration::{
    NativePayloadSchemas, NativeProtocolConnection, NativeSchemaBundle,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Message, protocol::Role},
};

/// Fixture recency: the thread's last update sits this far in the past.
const IDLE_FIXTURE_SECONDS: i64 = 90;
const TEST_SCRATCH: &str =
    "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000099";
/// Omits the effort key entirely when the caller requested none.
fn prompt_metadata(effort: Option<&str>) -> Value {
    effort.map_or_else(|| json!({}), |effort| json!({"effort": effort}))
}

fn ensure_test_scratch() {
    use std::os::unix::fs::PermissionsExt;
    assert!(std::fs::create_dir_all(TEST_SCRATCH).is_ok());
    assert!(std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).is_ok());
}

#[tokio::test]
async fn prompt_buffers_early_output_and_settles_native_completion_once() {
    ensure_test_scratch();
    // The fourth case omits the effort: the thread's own effort governs. The
    // fifth resumes a thread that kept "medium" while asking for "high".
    for (use_task, malformed_callback, requested_effort, resumed) in [
        (false, false, Some("medium"), false),
        (true, false, Some("medium"), false),
        (false, true, Some("medium"), false),
        (false, false, None, false),
        (false, false, Some("high"), true),
    ] {
        let mut catalog =
            AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
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
            serde_json::to_vec(&json!({"definitions":{
                "v2":definitions,
                "ServerRequest":{"type":"object","required":["id","method","params"],"properties":{
                    "id":{"type":["integer","string"]},"method":{"const":"item/commandExecution/requestApproval"},
                    "params":{"type":"object","required":["threadId","turnId","itemId"],"properties":{
                        "threadId":{"type":"string"},"turnId":{"type":"string"},"itemId":{"type":"string"},"command":{"type":["string","null"]}
                    }}
                }},
                "ServerNotification":{"type":"object","required":["method","params"],"properties":{
                    "method":{"enum":["item/agentMessage/delta","turn/completed"]},"params":{"type":"object"}
                }}
            }}))
                .unwrap_or_else(|error| panic!("schema JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("bundle: {error}"));
        let schemas = Arc::new(
            NativePayloadSchemas::from_bundle(&bundle)
                .unwrap_or_else(|error| panic!("schemas: {error}")),
        );
        let generation = serde_json::from_value(
            json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
        )
        .unwrap_or_else(|error| panic!("generation: {error}"));
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
        let connection = NativeProtocolConnection::from_websocket(
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        );
        // A thread that kept "high" proves resume echoes the persisted effort.
        let thread_effort = requested_effort.unwrap_or("high");
        // The effort the resumed thread already carried, before this turn.
        let persisted_effort = if resumed { "medium" } else { thread_effort };
        // The thread's own recency, not the prompt's, sets idleSeconds.
        let thread_updated_at = chrono::Utc::now().timestamp() - IDLE_FIXTURE_SECONDS;
        let thread_created_at = thread_updated_at - 600;
        let fixture = tokio::spawn(async move {
            let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let setup_method = if resumed {
                "thread/resume"
            } else {
                "thread/start"
            };
            for method in [setup_method, "turn/start"] {
                let frame = socket
                    .next()
                    .await
                    .unwrap_or_else(|| panic!("request"))
                    .unwrap_or_else(|error| panic!("frame: {error}"));
                let request: Value = serde_json::from_str(
                    frame
                        .to_text()
                        .unwrap_or_else(|error| panic!("text: {error}")),
                )
                .unwrap_or_else(|error| panic!("JSON: {error}"));
                assert_eq!(request["method"], method);
                let result = if method == setup_method {
                    if resumed {
                        // A thread created before routes were recorded: the native
                        // resume reports no settings and the broker holds no route.
                        json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":"thread-a","cwd":"/work","reasoningEffort":persisted_effort,"turns":[]}})
                    } else {
                        json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":{"type":"workspaceWrite","writableRoots":[TEST_SCRATCH]},"thread":{"id":"thread-a","cwd":"/work","reasoningEffort":persisted_effort,"turns":[]}})
                    }
                } else {
                    assert_eq!(request["params"]["input"][0]["text"], "hello");
                    // Without a requested effort the key is absent, not null.
                    assert_eq!(
                        request["params"].get("effort").and_then(Value::as_str),
                        requested_effort
                    );
                    socket.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"message-a","delta":"early output"}}).to_string().into())).await.unwrap_or_else(|error| panic!("early output: {error}"));
                    json!({"turn":{"id":"turn-a"}})
                };
                socket
                    .send(Message::Text(
                        json!({"id":request["id"],"result":result})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap_or_else(|error| panic!("response: {error}"));
            }
            socket.send(Message::Text(json!({"id":9007199254740993_i64,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"tool-a","command":if malformed_callback {json!(42)} else {json!("cargo test")},"availableDecisions":["accept","decline"]}}).to_string().into())).await.unwrap_or_else(|error| panic!("permission: {error}"));
            if malformed_callback {
                return;
            }
            let reply = socket
                .next()
                .await
                .unwrap_or_else(|| panic!("reply"))
                .unwrap_or_else(|error| panic!("reply frame: {error}"));
            let reply: Value = serde_json::from_str(
                reply
                    .to_text()
                    .unwrap_or_else(|error| panic!("reply text: {error}")),
            )
            .unwrap_or_else(|error| panic!("reply JSON: {error}"));
            assert_eq!(
                reply,
                json!({"id":9007199254740993_i64,"result":{"decision":"cancel"}})
            );
            socket.send(Message::Text(json!({"method":"turn/completed","params":{"threadId":"thread-a","turn":{"id":"turn-a","status":"completed"}}}).to_string().into())).await.unwrap_or_else(|error| panic!("complete: {error}"));
            let frame = socket
                .next()
                .await
                .unwrap_or_else(|| panic!("thread read"))
                .unwrap_or_else(|error| panic!("thread read frame: {error}"));
            let request: Value = serde_json::from_str(
                frame
                    .to_text()
                    .unwrap_or_else(|error| panic!("thread read text: {error}")),
            )
            .unwrap_or_else(|error| panic!("thread read JSON: {error}"));
            assert_eq!(request["method"], "thread/read");
            socket.send(Message::Text(json!({"id":request["id"],"result":{"thread":{"id":"thread-a","model":"gpt-5.6-sol","reasoningEffort":thread_effort,"createdAt":thread_created_at,"updatedAt":thread_updated_at,"sandbox":{"type":"workspaceWrite"},"approvalPolicy":"on-request","approvalsReviewer":"auto_review"}}}).to_string().into())).await.unwrap_or_else(|error| panic!("thread read response: {error}"));
        });
        let setup_inputs = SessionSetupInputs {
            connection,
            schemas,
            generation,
            params: if resumed {
                json!({"sessionId":"thread-a","cwd":"/work","mcpServers":[]})
            } else {
                json!({"cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write","scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}})
            },
            approval_broker: std::sync::Arc::new(codex_acp_adapter::RejectingApprovalBroker),
        };
        let session = if resumed {
            AcpSessionBinding::load_existing(&mut catalog, setup_inputs)
                .await
                .map(|(session, _history)| session)
        } else {
            AcpSessionBinding::create(&mut catalog, setup_inputs).await
        }
        .unwrap_or_else(|error| panic!("session: {error}"));
        if use_task {
            let closed = tokio_util::sync::CancellationToken::new();
            let (output, mut frames) = codex_acp_adapter::bounded_acp_output(closed.clone());
            let mut registry = codex_acp_adapter::AcpSessionRegistry::new(output, closed);
            registry
                .insert(session)
                .unwrap_or_else(|error| panic!("insert: {error}"));
            let params = json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"hello"}],"_meta":{"codexRouter":prompt_metadata(requested_effort)}});
            registry
                .begin_prompt(&mut catalog, json!("acp-prompt"), params.clone())
                .unwrap_or_else(|error| panic!("begin: {error}"));
            assert!(matches!(
                registry.begin_prompt(&mut catalog, json!("second"), params),
                Err(codex_acp_adapter::SessionRegistryError::Busy)
            ));
            let observed=tokio::time::timeout(std::time::Duration::from_secs(3),async {
                let update=frames.recv().await.unwrap_or_else(||panic!("update"));
                assert_eq!(update["params"]["update"]["content"]["text"],"early output");
                tokio::select! {
                    biased;
                    frame=frames.recv()=>panic!("terminal published before registry completion: {}", frame.is_some()),
                    result=registry.complete_next()=>result.unwrap_or_else(|error|panic!("completion: {error}")),
                }
                let terminal=frames.recv().await.unwrap_or_else(||panic!("terminal"));
                assert_eq!(terminal["result"]["stopReason"],"end_turn");
                assert!(!registry.has_pending());
            }).await;
            assert!(observed.is_ok(), "registry deadline");
            registry.shutdown().await;
        } else {
            let mut prompt = PendingAcpPrompt::start(
                session,
                &mut catalog,
                json!("acp-prompt"),
                &json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"hello"}],"_meta":{"codexRouter":prompt_metadata(requested_effort)}}),
            )
            .await
            .unwrap_or_else(|error| panic!("prompt: {error}"));
            let update = prompt
                .next_event(&mut catalog)
                .await
                .unwrap_or_else(|error| panic!("update: {error}"));
            assert!(
                matches!(update,Some(PromptEvent::Update(value)) if value["params"]["update"]["content"]["text"]=="early output")
            );
            if malformed_callback {
                assert!(
                    prompt.next_event(&mut catalog).await.is_err(),
                    "malformed native command must not become an ACP permission request"
                );
                assert!(prompt.blocks_next_prompt());
                fixture.await.unwrap();
                continue;
            }
            let decision = prompt
                .next_event(&mut catalog)
                .await
                .unwrap_or_else(|error| panic!("decision: {error}"));
            assert!(
                matches!(decision, Some(PromptEvent::NativeNotification(value)) if value["kind"] == "approvalDecisionSubmitted")
            );
            let terminal = prompt
                .next_event(&mut catalog)
                .await
                .unwrap_or_else(|error| panic!("terminal: {error}"));
            assert!(
                matches!(&terminal,Some(PromptEvent::Terminal(value)) if value["id"]=="acp-prompt" && value["result"]["stopReason"]=="end_turn")
            );
            let expected_access = if resumed {
                // A resumed thread with no recorded route inherits its access.
                Value::Null
            } else {
                Value::from("workspace-write")
            };
            let Some(PromptEvent::Terminal(terminal)) = &terminal else {
                panic!("terminal receipt")
            };
            assert_eq!(
                terminal["result"]["_meta"]["codexRouter"]["effectiveAccess"],
                expected_access
            );
            assert_eq!(
                terminal["result"]["_meta"]["codexRouter"]["effectiveEffort"], thread_effort,
                "the receipt reports the effort the thread actually carries"
            );
            let idle_seconds = terminal["result"]["_meta"]["codexRouter"]["idleSeconds"]
                .as_i64()
                .unwrap_or_else(|| panic!("idleSeconds"));
            assert!(
                (IDLE_FIXTURE_SECONDS..IDLE_FIXTURE_SECONDS + 60).contains(&idle_seconds),
                "idleSeconds {idle_seconds} must follow the thread's updatedAt"
            );
            assert!(!prompt.blocks_next_prompt());
            assert!(prompt.cancel().await.is_none());
        }
        fixture
            .await
            .unwrap_or_else(|error| panic!("fixture: {error}"));
    }
}
