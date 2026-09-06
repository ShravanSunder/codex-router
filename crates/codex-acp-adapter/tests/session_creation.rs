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

#[tokio::test]
async fn new_session_mints_scoped_configuration_receipt_and_checks_effective_cwd() {
    for (creating, effective_cwd) in [
        (true, "/work/project"),
        (true, "/wrong/project"),
        (false, "/work/project"),
        (false, "/wrong/project"),
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
            serde_json::to_vec(&json!({"definitions":{"v2":definitions}}))
                .unwrap_or_else(|error| panic!("JSON: {error}")),
        )]))
        .unwrap_or_else(|error| panic!("bundle: {error}"));
        let schemas = Arc::new(
            NativePayloadSchemas::from_bundle(&bundle)
                .unwrap_or_else(|error| panic!("schemas: {error}")),
        );
        let generation: communication_protocol::CodexGeneration = serde_json::from_value(
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
                assert_eq!(
                    request["params"]["config"]["mcp_servers"]["notes"]["args"],
                    json!(["--stdio"])
                );
            } else {
                assert_eq!(request["method"], "thread/resume");
                assert_eq!(request["params"], json!({"threadId":"new-thread"}));
            }
            server.send(Message::Text(json!({"id":request["id"],"result":{"cwd":effective_cwd,"thread":{"id":"new-thread","cwd":effective_cwd,"turns":[{"id":"old-turn","items":[{"type":"agentMessage","id":"message","text":"previous answer"}]}]}}}).to_string().into())).await.unwrap_or_else(|error| panic!("send: {error}"));
        });
        let result =
            if creating {
                AcpSessionBinding::create(
                    &mut catalog,
                    SessionSetupInputs {
                        connection,
                        schemas,
                        generation: generation.clone(),
                        params: json!({"cwd":"/work/project","mcpServers":[server_config]}),
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
                },
            )
            .await
            .map(|(session, history)| {
                assert_eq!(history.len(),1);
                assert_eq!(history[0]["params"]["update"]["content"]["text"],"previous answer");
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
