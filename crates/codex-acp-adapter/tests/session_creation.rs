use codex_acp_adapter::{
    AcpSchemaCatalog, AcpSessionBinding, McpConfiguration, SessionSetupError, SessionSetupInputs,
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

const TEST_SCRATCH: &str =
    "/tmp/router-acp-tests/scratch/session-00000000-0000-4000-8000-000000000099";
fn ensure_test_scratch() {
    use std::os::unix::fs::PermissionsExt;
    assert!(std::fs::create_dir_all(TEST_SCRATCH).is_ok());
    assert!(std::fs::set_permissions(TEST_SCRATCH, std::fs::Permissions::from_mode(0o700)).is_ok());
}

#[tokio::test]
async fn new_session_mints_scoped_configuration_receipt_and_checks_effective_cwd() {
    ensure_test_scratch();
    for (creating, effective_cwd, effective_reviewer) in [
        (true, "/work/project", "user"),
        (true, "/wrong/project", "user"),
        (false, "/work/project", "user"),
        (false, "/wrong/project", "user"),
    ] {
        let mut catalog =
            AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
        let mut definitions = serde_json::Map::new();
        for name in [
            "ThreadRead",
            "ThreadFork",
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
            serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                .unwrap_or_else(|error| panic!("JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("bundle: {error}"));
        let schemas = Arc::new(
            NativePayloadSchemas::from_bundle(&bundle)
                .unwrap_or_else(|error| panic!("schemas: {error}")),
        );
        let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
            json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
        )
        .unwrap_or_else(|error| panic!("generation: {error}"));
        let server_config =
            json!({"name":"notes","command":"/usr/bin/example","args":["--stdio"],"env":[]});
        let configuration =
            McpConfiguration::parse(&mut catalog, std::slice::from_ref(&server_config))
                .unwrap_or_else(|error| panic!("MCP: {error}"));
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
        let connection = NativeProtocolConnection::from_websocket(
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        );
        let fixture = tokio::spawn(async move {
            let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let frame = server
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|error| panic!("frame: {error}"));
            let request: Value = serde_json::from_str(
                frame
                    .to_text()
                    .unwrap_or_else(|error| panic!("text: {error}")),
            )
            .unwrap_or_else(|error| panic!("request JSON: {error}"));
            if creating {
                assert_eq!(request["method"], "thread/start");
                assert_eq!(request["params"]["cwd"], "/work/project");
                assert_eq!(request["params"]["model"], "gpt-5.6-sol");
                assert_eq!(
                    request["params"]["config"]["model_reasoning_effort"],
                    "medium"
                );
                assert_eq!(request["params"]["allowProviderModelFallback"], false);
                assert_eq!(request["params"]["threadSource"], "user");
                assert_eq!(request["params"]["permissions"], "router-write-restricted");
                assert_eq!(
                    request["params"]["config"]["permissions.router-write-restricted.extends"],
                    ":read-only"
                );
                assert_eq!(
                    request["params"]["config"]["permissions.router-write-restricted.filesystem"]
                        [TEST_SCRATCH],
                    "write"
                );
                assert!(
                    request["params"]["config"]
                        .get("permissions.router-write-restricted")
                        .is_none()
                );
                assert!(request["params"].get("sandbox").is_none());
                assert!(request["params"].get("approvalPolicy").is_none());
                assert!(request["params"].get("approvalsReviewer").is_none());
                assert_eq!(
                    request["params"]["config"]["mcp_servers"]["notes"]["args"],
                    json!(["--stdio"])
                );
            } else {
                assert_eq!(request["method"], "thread/resume");
                assert_eq!(request["params"], json!({"threadId":"new-thread"}));
            }
            server.send(Message::Text(json!({"id":request["id"],"result":{"cwd":effective_cwd,"model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":effective_reviewer,"activePermissionProfile":{"id":"router-write-restricted","extends":":read-only"},"sandbox":{"type":"workspaceWrite","writableRoots":[TEST_SCRATCH,"/work/project/docs/wip","/work/project/tmp"],"excludeTmpdirEnvVar":true,"excludeSlashTmp":true},"thread":{"id":"new-thread","cwd":effective_cwd,"turns":[{"id":"old-turn","items":[{"type":"agentMessage","id":"message","text":"previous answer"}]}]}}}).to_string().into())).await.unwrap_or_else(|error| panic!("send: {error}"));
        });
        let result = if creating {
            AcpSessionBinding::create(
                    &mut catalog,
                    SessionSetupInputs {
                        connection,
                        schemas,
                        generation: generation.clone(),
                        params: json!({"cwd":"/work/project","mcpServers":[server_config],"_meta":{"codexRouter":{"model":"gpt-5.6-sol","effort":"medium","access":"write-restricted","scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}}),
                        approval_broker: std::sync::Arc::new(codex_acp_adapter::RejectingApprovalBroker),
                    },
                )
                .await
        } else {
            AcpSessionBinding::load_existing(
                &mut catalog,
                SessionSetupInputs {
                    connection,
                    schemas,
                    generation: generation.clone(),
                    params: json!({"sessionId":"new-thread","cwd":"/work/project","mcpServers":[]}),
                    approval_broker: std::sync::Arc::new(
                        codex_acp_adapter::RejectingApprovalBroker,
                    ),
                },
            )
            .await
            .map(|(session, history)| {
                assert_eq!(history.len(), 1);
                assert_eq!(
                    history[0]["params"]["update"]["content"]["text"],
                    "previous answer"
                );
                session
            })
        };
        fixture
            .await
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        if effective_cwd == "/work/project" {
            let mut session = result.unwrap_or_else(|error| panic!("create: {error}"));
            assert_eq!(
                session.new_session_result(),
                json!({"sessionId":"new-thread"})
            );
            assert_eq!(
                session.accepts_configuration(&generation, "new-thread", &configuration),
                creating
            );
            assert!(!session.accepts_configuration(&generation, "other-thread", &configuration));
            let mut replaced = generation.clone();
            replaced.generation = 2_u64
                .try_into()
                .unwrap_or_else(|error| panic!("replacement: {error}"));
            assert!(!session.accepts_configuration(&replaced, "new-thread", &configuration));
            let mismatched = session
                .resume_with_receipt(
                    &mut catalog,
                    &replaced,
                    &json!({"sessionId":"new-thread","cwd":"/work/project","mcpServers":[]}),
                )
                .await;
            assert!(matches!(
                mismatched,
                Err(SessionSetupError::ConfigurationMismatch)
            ));
            let changed_settings = session.resume_with_receipt(&mut catalog, &generation, &json!({"sessionId":"new-thread","cwd":"/work/project","mcpServers":[{"name":"other","command":"/usr/bin/example","args":[],"env":[]}]})).await;
            assert!(matches!(
                changed_settings,
                Err(SessionSetupError::ConfigurationMismatch)
            ));
        } else {
            assert!(matches!(
                result,
                Err(SessionSetupError::ConfigurationMismatch)
            ));
        }
    }
}

#[tokio::test]
async fn fork_session_sends_exact_model_choice_to_native_runtime() {
    ensure_test_scratch();
    // The second case omits both choices: the source thread's own values govern.
    for explicit_choice in [true, false] {
        let mut catalog =
            AcpSchemaCatalog::load().unwrap_or_else(|error| panic!("catalog: {error}"));
        let mut definitions = serde_json::Map::new();
        for name in [
            "ThreadFork",
            "ThreadLoadedList",
            "ThreadRead",
            "ThreadResume",
            "ThreadStart",
            "TurnInterrupt",
            "TurnStart",
            "TurnSteer",
        ] {
            definitions.insert(format!("{name}Params"), json!({"type":"object"}));
            definitions.insert(format!("{name}Response"), json!({"type":"object"}));
        }
        let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
            "codex_app_server_protocol.schemas.json".to_owned(),
            serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                .unwrap_or_else(|error| panic!("JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("bundle: {error}"));
        let schemas = Arc::new(
            NativePayloadSchemas::from_bundle(&bundle)
                .unwrap_or_else(|error| panic!("schemas: {error}")),
        );
        let generation: collaboration_protocol::CodexGeneration = serde_json::from_value(
            json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
        )
        .unwrap_or_else(|error| panic!("generation: {error}"));
        let (client, server) =
            tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
        let connection = NativeProtocolConnection::from_websocket(
            WebSocketStream::from_raw_socket(client, Role::Client, None).await,
        );
        let fixture = tokio::spawn(async move {
            let mut server = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
            let frame = server
                .next()
                .await
                .unwrap_or_else(|| panic!("request"))
                .unwrap_or_else(|error| panic!("frame: {error}"));
            let mut request: Value = serde_json::from_str(
                frame
                    .to_text()
                    .unwrap_or_else(|error| panic!("text: {error}")),
            )
            .unwrap_or_else(|error| panic!("request JSON: {error}"));
            if !explicit_choice {
                // The source thread is read once, before the fork is dispatched.
                assert_eq!(request["method"], "thread/read");
                assert_eq!(request["params"]["threadId"], "source-thread");
                assert_eq!(request["params"]["includeTurns"], false);
                server
                .send(Message::Text(
                    json!({"id":request["id"],"result":{"thread":{"id":"source-thread","model":"gpt-6-astra","reasoningEffort":"high"}}})
                        .to_string()
                        .into(),
                ))
                .await
                .unwrap_or_else(|error| panic!("source read: {error}"));
                let frame = server
                    .next()
                    .await
                    .unwrap_or_else(|| panic!("fork request"))
                    .unwrap_or_else(|error| panic!("fork frame: {error}"));
                request = serde_json::from_str(
                    frame
                        .to_text()
                        .unwrap_or_else(|error| panic!("fork text: {error}")),
                )
                .unwrap_or_else(|error| panic!("fork JSON: {error}"));
            }
            assert_eq!(request["method"], "thread/fork");
            assert_eq!(request["params"]["threadId"], "source-thread");
            assert_eq!(request["params"]["excludeTurns"], true);
            assert_eq!(request["params"]["cwd"], "/work/project");
            assert_eq!(request["params"]["model"], "gpt-6-astra");
            assert_eq!(
                request["params"]["config"]["model_reasoning_effort"],
                "high"
            );
            assert_eq!(request["params"]["allowProviderModelFallback"], false);
            assert_eq!(request["params"]["threadSource"], "user");
            assert_eq!(request["params"]["permissions"], "router-workspace-write");
            assert!(request["params"].get("sandbox").is_none());
            assert!(request["params"].get("approvalPolicy").is_none());
            assert!(request["params"].get("approvalsReviewer").is_none());
            server
            .send(Message::Text(
                json!({
                    "id": request["id"],
                    "result": {
                        "cwd": "/work/project",
                        "model": "gpt-6-astra",
                        "approvalPolicy": "on-request",
                        "approvalsReviewer": "auto_review",
                        "activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},
                        "sandbox":{"type":"workspaceWrite","writableRoots":[TEST_SCRATCH]},
                        "thread": {
                            "id": "forked-thread",
                            "cwd": "/work/project",
                            "turns": []
                        }
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        });

        let session = AcpSessionBinding::create(
        &mut catalog,
        SessionSetupInputs {
            connection,
            schemas,
            generation,
            params: json!({
                "cwd": "/work/project",
                "mcpServers": [],
                "_meta": {
                    "codexRouter": if explicit_choice { json!({
                        "model": "gpt-6-astra",
                        "effort": "high",
                        "access": "workspace-write",
                        "scratchScope":"session-00000000-0000-4000-8000-000000000099",
                        "scratchPath":TEST_SCRATCH,
                        "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
                        "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
                        "forkThreadId": "source-thread"
                    }) } else { json!({
                        "access": "workspace-write",
                        "scratchScope":"session-00000000-0000-4000-8000-000000000099",
                        "scratchPath":TEST_SCRATCH,
                        "createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
                        "approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},
                        "forkThreadId": "source-thread"
                    }) }
                }
            }),
            approval_broker: std::sync::Arc::new(
                codex_acp_adapter::RejectingApprovalBroker,
            ),
        },
    )
    .await
    .unwrap_or_else(|error| panic!("fork: {error}"));
        fixture
            .await
            .unwrap_or_else(|error| panic!("fixture: {error}"));
        assert_eq!(session.session_id(), "forked-thread");
    }
}
