//! The common cancellation tool must not turn Codex ACP work into provider cancellation.
use crate::api_test_harness::{ServedApi, api_config};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn codex_conversation_cancel_names_turn_interrupt_without_sending_acp_cancel() {
    let root = tempfile::tempdir().expect("temporary service directory");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private service directory");
    let service_id = "00000000-0000-4000-8000-000000000001";
    let epoch = "00000000-0000-4000-8000-000000000002";
    let endpoint = json!({"serviceId":service_id,"endpointId":"codex-local"});
    let description = serde_json::from_value(json!({
        "endpoint":endpoint,"label":"Fixture Codex",
        "availability":{"state":"available","observedAt":"2026-09-24T00:00:00Z"},
        "channels":[{"kind":"acp","transport":"unixJsonLines","path":"codex-acp.sock",
            "schemaDigest":format!("sha256:{}", collaboration_protocol::ACP_SCHEMA_DIGEST)}]
    }))
    .expect("Codex endpoint description");
    let identity = collaboration_service::ServiceIdentity::new(service_id, epoch)
        .expect("service identity")
        .with_endpoints(vec![description])
        .expect("endpoint directory");
    // The carrier tools discover the ACP socket from the Router's own endpoint directory.
    let application = collaboration_service::CollaborationApplication::new(identity);
    let acp =
        tokio::net::UnixListener::bind(root.path().join("codex-acp.sock")).expect("ACP listener");
    let acp_peer = tokio::spawn(async move {
        for operation in ["cancel", "prompt", "load"] {
            let (stream, _) = acp.accept().await.expect("ACP accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            let initialize: Value = serde_json::from_str(
                &lines
                    .next_line()
                    .await
                    .expect("ACP initialize read")
                    .expect("ACP initialize frame"),
            )
            .expect("ACP initialize JSON");
            assert_eq!(initialize["method"], "initialize");
            writer
            .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":initialize["id"],
                "result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}})).as_bytes())
            .await
            .expect("ACP initialize response");
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                    .await
                    .expect("ACP caller closes promptly")
                    .expect("ACP EOF read")
                    .is_none(),
                "unsupported {operation} must not send ACP input"
            );
        }
    });
    let listener = ServedApi::tcp(&api_config(application, root.path())).await;
    let client = reqwest::Client::new();
    let target = json!({"endpoint":endpoint,"sessionId":"codex-thread"});
    let response = client
        .post(listener.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(
            &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"conversation_cancel","arguments":{
                "operationId":collaboration_protocol::OperationId::generate(),
                "targetOperationId":collaboration_protocol::OperationId::generate(),
                "target":target,"requestedBy":target
            }}}),
        )
        .send()
        .await
        .expect("MCP cancel response");
    let body = response.text().await.expect("MCP body");
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.is_empty())
        .unwrap_or(&body);
    let result: Value = serde_json::from_str(encoded).expect("MCP JSON");
    assert_eq!(result["result"]["isError"], true, "{result}");
    assert_eq!(
        result["result"]["structuredContent"]["kind"],
        "unsupportedCapability"
    );
    assert_eq!(result["result"]["structuredContent"]["field"], "cancel");
    assert!(
        result["result"]["structuredContent"]["fix"]
            .as_str()
            .is_some_and(|fix| fix.contains("turn interrupt"))
    );
    let prompt_operation_id = collaboration_protocol::OperationId::generate();
    let response = client
        .post(listener.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
            "name":"conversation_prompt","arguments":{
                "operationId":prompt_operation_id,"target":target,
                "workingDirectory":root.path(),"requestedBy":target,
                "message":{"kind":"humanUser","text":"should not submit"},
                "timeoutSeconds":1
            }}}),
        )
        .send()
        .await
        .expect("MCP prompt response");
    let body = response.text().await.expect("MCP prompt body");
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.is_empty())
        .unwrap_or(&body);
    let prompt: Value = serde_json::from_str(encoded).expect("MCP prompt JSON");
    assert_eq!(prompt["result"]["isError"], true, "{prompt}");
    assert_eq!(
        prompt["result"]["structuredContent"]["kind"],
        "unsupportedCapability"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["field"],
        "operationId"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["operationId"],
        json!(prompt_operation_id)
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["fix"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );
    let load_operation_id = collaboration_protocol::OperationId::generate();
    let response = client
        .post(listener.url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-11-25")
        .json(
            &json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
            "name":"conversation_load","arguments":{
                "operationId":load_operation_id,"target":target,
                "workingDirectory":root.path(),"requestedBy":target,
                "access":"workspace-write","timeoutSeconds":1
            }}}),
        )
        .send()
        .await
        .expect("MCP load response");
    let body = response.text().await.expect("MCP load body");
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|line| !line.is_empty())
        .unwrap_or(&body);
    let load: Value = serde_json::from_str(encoded).expect("MCP load JSON");
    assert_eq!(load["result"]["isError"], true, "{load}");
    assert_eq!(
        load["result"]["structuredContent"]["kind"],
        "unsupportedCapability"
    );
    assert_eq!(load["result"]["structuredContent"]["field"], "operationId");
    assert_eq!(
        load["result"]["structuredContent"]["operationId"],
        json!(load_operation_id)
    );
    assert_eq!(
        load["result"]["structuredContent"]["fix"],
        "omit the operation ID for Codex prompts; it is not inspectable"
    );
    acp_peer.await.expect("ACP peer");
    listener.stop().await;
}
