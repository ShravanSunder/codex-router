use codex_acp_adapter::{
    AcpSchemaCatalog, AcpSessionBinding, HeldBindingCheckout, PendingAcpPrompt, PromptEvent,
    SessionSetupInputs, UnmaterializedBindingStore,
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
#[path = "support/native_permission_echo.rs"]
mod native_permission_echo;
use native_permission_echo::applied_router_sandbox;
#[path = "support/conversation_operation_recorder.rs"]
mod conversation_operation_recorder;
use conversation_operation_recorder::AcceptingConversationRecorder;

/// Fixture recency: the thread's last update sits this far in the past.
const IDLE_FIXTURE_SECONDS: i64 = 90;
struct TestBindingHolder;
impl UnmaterializedBindingStore for TestBindingHolder {
    fn hold(&self, _binding: AcpSessionBinding) {}
    fn checkout(&self, _session_id: &str) -> HeldBindingCheckout {
        HeldBindingCheckout::Missing
    }
    fn restore(&self, _binding: AcpSessionBinding) {}
    fn finish(&self, _session_id: &str) {}
    fn host_tasks(&self) -> tokio_util::task::TaskTracker {
        tokio_util::task::TaskTracker::new()
    }
}
const TEST_SCRATCH: &str =
    "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000099";
#[derive(Clone, Copy, PartialEq, Eq)]
enum FinalReplyFixture {
    Available,
    Invalid,
    Oversized,
    NoReply,
    Commentary,
    CancelBeforeText,
    PlanOnly,
    PlanWithProse,
}
#[path = "native_prompt_execution/early_cancellation.rs"]
mod early_cancellation;
fn expected_final_reply(fixture: FinalReplyFixture) -> Value {
    match fixture {
        FinalReplyFixture::Available => json!({"kind":"available","text":"selected answer"}),
        FinalReplyFixture::Invalid => {
            json!({"kind":"unavailable","reason":"outputInvalid"})
        }
        FinalReplyFixture::Oversized => {
            json!({"kind":"unavailable","reason":"outputLimitExceeded"})
        }
        FinalReplyFixture::NoReply
        | FinalReplyFixture::Commentary
        | FinalReplyFixture::CancelBeforeText => {
            json!({"kind":"available","text":null})
        }
        FinalReplyFixture::PlanOnly => json!({"kind":"available","text":"first plan"}),
        FinalReplyFixture::PlanWithProse => {
            json!({"kind":"available","text":"last completed plan"})
        }
    }
}
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
    // The fourth case omits effort; the fifth resumes with the inherited
    // effort. Later cases prove invalid output does not short-circuit callbacks.
    for (use_task, malformed_callback, requested_effort, resumed, reply, cancel) in [
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::Available,
            false,
        ),
        (
            true,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::Available,
            false,
        ),
        (
            false,
            true,
            Some("medium"),
            false,
            FinalReplyFixture::Available,
            false,
        ),
        (
            false,
            false,
            None,
            false,
            FinalReplyFixture::Available,
            false,
        ),
        (
            false,
            false,
            Some("high"),
            true,
            FinalReplyFixture::Available,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::Invalid,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::Oversized,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::NoReply,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::Commentary,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::PlanOnly,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::PlanWithProse,
            false,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::NoReply,
            true,
        ),
        (
            false,
            false,
            Some("medium"),
            false,
            FinalReplyFixture::CancelBeforeText,
            true,
        ),
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
                    "method":{"enum":["item/agentMessage/delta","item/completed","turn/completed"]},"params":{"type":"object"}
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
        let completed_reply = match reply {
            FinalReplyFixture::Available => Some("selected answer".to_owned()),
            FinalReplyFixture::Invalid => Some("before\u{0001}after".to_owned()),
            FinalReplyFixture::Oversized => Some("x".repeat(1_048_577)),
            FinalReplyFixture::NoReply => None,
            FinalReplyFixture::Commentary => Some("commentary only".to_owned()),
            FinalReplyFixture::CancelBeforeText | FinalReplyFixture::PlanOnly => None,
            FinalReplyFixture::PlanWithProse => Some("surrounding prose".to_owned()),
        };
        let reply_phase = match reply {
            FinalReplyFixture::Commentary => "commentary",
            _ => "final_answer",
        };
        let cancel = cancel || matches!(reply, FinalReplyFixture::CancelBeforeText);
        let has_early_delta = !matches!(
            reply,
            FinalReplyFixture::PlanOnly | FinalReplyFixture::CancelBeforeText
        );
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
            for method in [setup_method, "thread/read", "turn/start"] {
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
                let result = if method == "thread/read" {
                    json!({"thread":{"id":"thread-a","status":{"type":"idle"},"turns":[]}})
                } else if method == setup_method {
                    if resumed {
                        // A thread created before routes were recorded: the native
                        // resume reports no settings and the broker holds no route.
                        json!({"cwd":"/work","model":"gpt-5.6-sol","thread":{"id":"thread-a","cwd":"/work","status":{"type":"idle"},"reasoningEffort":persisted_effort,"turns":[]}})
                    } else {
                        json!({"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":applied_router_sandbox(&request),"thread":{"id":"thread-a","cwd":"/work","status":{"type":"idle"},"reasoningEffort":persisted_effort,"turns":[]}})
                    }
                } else {
                    assert_eq!(request["params"]["input"][0]["text"], "hello");
                    // Without a requested effort the key is absent, not null.
                    assert_eq!(
                        request["params"].get("effort").and_then(Value::as_str),
                        requested_effort
                    );
                    if has_early_delta {
                        socket.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"message-a","delta":"early output"}}).to_string().into())).await.unwrap_or_else(|error| panic!("early output: {error}"));
                    }
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
            if matches!(
                reply,
                FinalReplyFixture::PlanOnly | FinalReplyFixture::PlanWithProse
            ) {
                socket.send(Message::Text(json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"plan","id":"plan-first","text":"first plan"}}}).to_string().into())).await.unwrap_or_else(|error| panic!("first plan: {error}"));
            }
            if let Some(text) = completed_reply.as_ref()
                && reply != FinalReplyFixture::PlanWithProse
            {
                socket.send(Message::Text(json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"agentMessage","id":"final-a","phase":reply_phase,"text":text}}}).to_string().into())).await.unwrap_or_else(|error| panic!("completed message: {error}"));
            }
            if reply == FinalReplyFixture::PlanWithProse {
                socket.send(Message::Text(json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"plan","id":"plan-last","text":"last completed plan"}}}).to_string().into())).await.unwrap_or_else(|error| panic!("last plan: {error}"));
                let text = completed_reply
                    .as_ref()
                    .unwrap_or_else(|| panic!("plan fixture prose"));
                socket.send(Message::Text(json!({"method":"item/completed","params":{"threadId":"thread-a","turnId":"turn-a","item":{"type":"agentMessage","id":"prose-after-plan","phase":"final_answer","text":text}}}).to_string().into())).await.unwrap_or_else(|error| panic!("prose after last plan: {error}"));
            }
            if matches!(
                reply,
                FinalReplyFixture::Invalid | FinalReplyFixture::Oversized
            ) {
                socket.send(Message::Text(json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-a","turnId":"turn-a","itemId":"message-after-reply","delta":"continued after unavailable reply"}}).to_string().into())).await.unwrap_or_else(|error| panic!("continuation after unavailable reply: {error}"));
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
            if cancel {
                let interrupt = socket.next().await.unwrap().unwrap();
                let request: Value = serde_json::from_str(interrupt.to_text().unwrap()).unwrap();
                assert_eq!(request["method"], "turn/interrupt");
                socket.send(Message::Text(json!({"id":request["id"],"result":{"turn":{"id":"turn-a","status":"interrupted"}}}).to_string().into())).await.unwrap();
                return;
            }
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
            operation_id: None,
            recorder: Arc::new(AcceptingConversationRecorder),
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
            let mut registry = codex_acp_adapter::AcpSessionRegistry::new(
                output,
                closed,
                Arc::new(TestBindingHolder),
            );
            registry
                .insert(session)
                .unwrap_or_else(|(reason, _binding)| panic!("insert: {reason}"));
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
                assert_eq!(terminal["result"]["_meta"]["codex-router/finalReply"],expected_final_reply(reply));
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
            if has_early_delta {
                let update = prompt
                    .next_event(&mut catalog)
                    .await
                    .unwrap_or_else(|error| panic!("update: {error}"));
                assert!(
                    matches!(update,Some(PromptEvent::Update(value)) if value["params"]["update"]["content"]["text"]=="early output")
                );
            }
            if matches!(reply, FinalReplyFixture::PlanOnly) {
                assert!(matches!(
                    prompt.next_event(&mut catalog).await.unwrap(),
                    Some(PromptEvent::NativeNotification(value))
                        if value["params"]["item"]["type"] == "plan"
                ));
            } else if matches!(reply, FinalReplyFixture::PlanWithProse) {
                for expected_type in ["plan", "plan", "agentMessage"] {
                    assert!(matches!(
                        prompt.next_event(&mut catalog).await.unwrap(),
                        Some(PromptEvent::NativeNotification(value))
                            if value["params"]["item"]["type"] == expected_type
                    ));
                }
            } else if !matches!(
                reply,
                FinalReplyFixture::NoReply | FinalReplyFixture::CancelBeforeText
            ) {
                assert!(matches!(
                    prompt.next_event(&mut catalog).await.unwrap(),
                    Some(PromptEvent::NativeNotification(value))
                        if value["method"] == "item/completed"
                ));
            }
            if matches!(
                reply,
                FinalReplyFixture::Invalid | FinalReplyFixture::Oversized
            ) {
                assert!(matches!(
                    prompt.next_event(&mut catalog).await.unwrap(),
                    Some(PromptEvent::Update(value))
                        if value["params"]["update"]["content"]["text"] == "continued after unavailable reply"
                ));
            }
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
            if cancel {
                let Some(response) = prompt.cancel().await else {
                    panic!("cancel response")
                };
                assert_eq!(response["result"]["stopReason"], "cancelled");
                assert_eq!(
                    response["result"]["_meta"]["codex-router/nativeInterruption"]["state"],
                    "confirmed"
                );
                assert_eq!(
                    response["result"]["_meta"]["codex-router/finalReply"],
                    json!({"kind":"available","text":null})
                );
                fixture.await.unwrap();
                continue;
            }
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
                terminal["result"]["_meta"]["codex-router/finalReply"],
                expected_final_reply(reply)
            );
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
