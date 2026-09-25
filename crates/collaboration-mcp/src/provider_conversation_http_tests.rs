use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(super) const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
pub(super) const CREATE_OPERATION: &str = "019f0000-0000-7000-8000-000000000001";
const PROMPT_OPERATION: &str = "019f0000-0000-7000-8000-000000000002";
pub(super) const WAIT_OPERATION: &str = "019f0000-0000-7000-8000-000000000003";
const LOAD_OPERATION: &str = "019f0000-0000-7000-8000-000000000004";
const PENDING_OPERATION: &str = "019f0000-0000-7000-8000-000000000005";
const CANCEL_OPERATION: &str = "019f0000-0000-7000-8000-000000000006";

#[tokio::test]
async fn initialized_http_exposes_one_conversation_surface_for_provider_operations() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let peer = tokio::spawn(async move {
        for expected in [
            "conversation/create",
            "conversation/prompt",
            "conversation/operationShow",
            "conversation/load",
        ] {
            let (stream, _) = control.accept().await.expect("Control accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            initialize_control(&mut lines, &mut writer).await;
            if expected != "conversation/operationShow" {
                let inventory = read_json_line(&mut lines).await;
                assert_eq!(inventory["method"], "endpoint/list");
                write_json_line(
                    &mut writer,
                    json!({"jsonrpc":"2.0","id":inventory["id"],"result":endpoint_inventory()}),
                )
                .await;
            }
            let request = read_json_line(&mut lines).await;
            assert_eq!(request["method"], expected);
            let operation_id = request["params"]["operationId"]
                .as_str()
                .expect("operation ID");
            if expected == "conversation/load" {
                write_json_line(
                    &mut writer,
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{
                        "code":-32050,
                        "message":"provider binding is busy",
                        "data":{
                            "kind":"busy",
                            "stage":"admission",
                            "effect":"none",
                            "message":"provider binding is busy",
                            "operationId":operation_id,
                            "target":null
                        }
                    }}),
                )
                .await;
                continue;
            }
            let operation = match expected {
                "conversation/create" => "conversationCreate",
                "conversation/prompt" => "conversationPrompt",
                _ => "conversationPrompt",
            };
            let snapshot = if expected == "conversation/create" {
                terminal_snapshot(operation_id, operation)
            } else {
                operation_snapshot(operation_id, operation)
            };
            let result = if expected == "conversation/operationShow" {
                snapshot
            } else {
                json!({"admission":"admitted","operation":snapshot})
            };
            write_json_line(
                &mut writer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
            )
            .await;
            if expected == "conversation/prompt" {
                let wait = read_json_line(&mut lines).await;
                assert_eq!(wait["method"], "conversation/operationWait");
                assert_eq!(wait["params"]["operationId"], PROMPT_OPERATION);
                write_json_line(
                    &mut writer,
                    json!({"jsonrpc":"2.0","id":wait["id"],"result":{
                        "operation":terminal_snapshot(PROMPT_OPERATION,"conversationPrompt"),
                        "output":{"kind":"available","settlement":{
                            "kind":"promptCompleted",
                            "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
                            "stopReason":"end_turn","response":"provider reply"
                        }}
                    }}),
                )
                .await;
            }
        }
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;

    let tools = call_mcp(&client, &listener, &session, 2, "tools/list", json!({})).await;
    let listed = tools["result"]["tools"].as_array().expect("tool array");
    for name in [
        "conversation_create",
        "conversation_load",
        "conversation_prompt",
        "conversation_cancel",
        "conversation_operation_show",
        "conversation_operation_wait",
        "conversation_operation_reconcile",
    ] {
        let tool = listed
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing conversation tool {name}"));
        if matches!(name, "conversation_create" | "conversation_cancel") {
            assert!(
                tool["inputSchema"]["required"]
                    .as_array()
                    .expect("required inputs")
                    .contains(&json!("operationId")),
                "{name} must require the outer caller's operationId"
            );
        }
    }

    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let create = call_tool(
        &client,
        &listener,
        &session,
        3,
        "conversation_create",
        json!({
            "operationId":CREATE_OPERATION,
            "endpoint":endpoint,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "createdBy":actor,
            "approver":actor,
            "access":"workspace-write"
        }),
    )
    .await;
    assert_eq!(create["result"]["isError"], false, "{create}");
    assert_eq!(
        create["result"]["structuredContent"]["operationId"],
        CREATE_OPERATION
    );

    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let prompt = call_tool(
        &client,
        &listener,
        &session,
        4,
        "conversation_prompt",
        json!({
            "operationId":PROMPT_OPERATION,
            "target":target,
            "generation":generation(),
            "requestedBy":actor,
            "approver":actor,
            "message":{"kind":"humanUser","text":"continue the assignment"}
        }),
    )
    .await;
    assert_eq!(prompt["result"]["isError"], false);
    assert_eq!(prompt["result"]["structuredContent"]["kind"], "completed");
    assert_eq!(
        prompt["result"]["structuredContent"]["operationId"],
        PROMPT_OPERATION
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["stopReason"],
        "endTurn"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["kind"],
        "providerPrompt"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["output"]["kind"],
        "available"
    );
    assert_eq!(
        prompt["result"]["structuredContent"]["settlement"]["detail"]["output"]["text"],
        "provider reply"
    );

    let show = call_tool(
        &client,
        &listener,
        &session,
        5,
        "conversation_operation_show",
        json!({"operationId":PROMPT_OPERATION}),
    )
    .await;
    assert_eq!(show["result"]["isError"], false);
    assert_eq!(
        show["result"]["structuredContent"]["operationId"],
        PROMPT_OPERATION
    );

    let load = call_tool(
        &client,
        &listener,
        &session,
        6,
        "conversation_load",
        json!({
            "operationId":LOAD_OPERATION,
            "target":target,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "requestedBy":actor,
            "approver":actor,
            "access":"workspace-write"
        }),
    )
    .await;
    assert_eq!(load["result"]["isError"], true);
    assert_eq!(load["result"]["structuredContent"]["kind"], "busy");
    assert_eq!(
        load["result"]["structuredContent"]["operationId"],
        LOAD_OPERATION
    );
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn provider_prompt_wait_timeout_returns_pending_exact_operation_and_target() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        initialize_control(&mut lines, &mut writer).await;
        let inventory = read_json_line(&mut lines).await;
        assert_eq!(inventory["method"], "endpoint/list");
        write_json_line(
            &mut writer,
            json!({"jsonrpc":"2.0","id":inventory["id"],"result":endpoint_inventory()}),
        )
        .await;
        let prompt = read_json_line(&mut lines).await;
        assert_eq!(prompt["method"], "conversation/prompt");
        assert_eq!(prompt["params"]["operationId"], PENDING_OPERATION);
        let snapshot = operation_snapshot(PENDING_OPERATION, "conversationPrompt");
        write_json_line(
            &mut writer,
            json!({"jsonrpc":"2.0","id":prompt["id"],"result":{
                "admission":"admitted","operation":snapshot.clone()
            }}),
        )
        .await;
        let wait = read_json_line(&mut lines).await;
        assert_eq!(wait["method"], "conversation/operationWait");
        assert_eq!(wait["params"]["operationId"], PENDING_OPERATION);
        write_json_line(
            &mut writer,
            json!({"jsonrpc":"2.0","id":wait["id"],"result":{
                "operation":snapshot,"output":{"kind":"pending"}
            }}),
        )
        .await;
        assert!(
            lines
                .next_line()
                .await
                .expect("call-local connection read")
                .is_none(),
            "a pending prompt must not send conversation/cancel or retry"
        );
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        &session,
        2,
        "conversation_prompt",
        json!({
            "operationId":PENDING_OPERATION,"target":target,
            "requestedBy":actor,"approver":actor,
            "message":{"kind":"humanUser","text":"pending proof"},
            "timeoutSeconds":1
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(response["result"]["structuredContent"]["kind"], "pending");
    assert_eq!(
        response["result"]["structuredContent"]["operationId"],
        PENDING_OPERATION
    );
    assert_eq!(response["result"]["structuredContent"]["target"], target);
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn provider_prompt_without_operation_id_names_uuidv7_generator_before_mutation() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let peer = tokio::spawn(async move {
        for operation in ["prompt", "load"] {
            let (stream, _) = control.accept().await.expect("Control accept");
            let (reader, mut writer) = stream.into_split();
            let mut lines = BufReader::new(reader).lines();
            initialize_control(&mut lines, &mut writer).await;
            let inventory = read_json_line(&mut lines).await;
            assert_eq!(inventory["method"], "endpoint/list");
            write_json_line(
                &mut writer,
                json!({"jsonrpc":"2.0","id":inventory["id"],"result":endpoint_inventory()}),
            )
            .await;
            assert!(
                lines
                    .next_line()
                    .await
                    .expect("call-local connection read")
                    .is_none(),
                "a provider {operation} without operationId must not mutate the provider"
            );
        }
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        &session,
        2,
        "conversation_prompt",
        json!({"target":target,"requestedBy":actor,
            "message":{"kind":"humanUser","text":"must not submit"},"timeoutSeconds":1}),
    )
    .await;
    assert_eq!(response["result"]["isError"], true, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["kind"],
        "invalidRequest"
    );
    assert_eq!(response["result"]["structuredContent"]["effect"], "none");
    assert_eq!(
        response["result"]["structuredContent"]["operation"],
        "prompt"
    );
    let message = response["result"]["structuredContent"]["message"]
        .as_str()
        .expect("provider operation ID message");
    assert!(
        message.contains("canonical lowercase RFC UUIDv7"),
        "{message}"
    );
    let fix = response["result"]["structuredContent"]["fix"]
        .as_str()
        .expect("generator fix");
    assert!(
        fix.contains("python3 -c 'import uuid; print(uuid.uuid7())'"),
        "{fix}"
    );
    let load = call_tool(
        &client,
        &listener,
        &session,
        3,
        "conversation_load",
        json!({"target":target,"workingDirectory":temporary.path(),
            "requestedBy":actor,"access":"workspace-write","timeoutSeconds":1}),
    )
    .await;
    assert_eq!(load["result"]["isError"], true, "{load}");
    assert_eq!(
        load["result"]["structuredContent"]["kind"],
        "invalidRequest"
    );
    assert_eq!(load["result"]["structuredContent"]["effect"], "none");
    assert_eq!(load["result"]["structuredContent"]["operation"], "load");
    assert!(
        load["result"]["structuredContent"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("UUIDv7"))
    );
    assert!(
        load["result"]["structuredContent"]["fix"]
            .as_str()
            .is_some_and(|fix| fix.contains("python3 -c 'import uuid; print(uuid.uuid7())'"))
    );
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn provider_cancel_keeps_the_exact_target_operation_on_the_common_mcp_surface() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        initialize_control(&mut lines, &mut writer).await;
        let inventory = read_json_line(&mut lines).await;
        assert_eq!(inventory["method"], "endpoint/list");
        write_json_line(
            &mut writer,
            json!({"jsonrpc":"2.0","id":inventory["id"],"result":endpoint_inventory()}),
        )
        .await;
        let cancel = read_json_line(&mut lines).await;
        assert_eq!(cancel["method"], "conversation/cancel");
        assert_eq!(cancel["params"]["operationId"], CANCEL_OPERATION);
        assert_eq!(cancel["params"]["targetOperationId"], PROMPT_OPERATION);
        assert_eq!(cancel["params"]["target"]["sessionId"], "provider-thread");
        write_json_line(
            &mut writer,
            json!({"jsonrpc":"2.0","id":cancel["id"],"result":{
                "admission":"admitted",
                "operation":operation_snapshot(CANCEL_OPERATION,"conversationCancel")
            }}),
        )
        .await;
        assert!(
            lines
                .next_line()
                .await
                .expect("call-local connection read")
                .is_none(),
            "cancel is one exact mutation, not a replay or wait"
        );
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let response = call_tool(
        &client,
        &listener,
        &session,
        2,
        "conversation_cancel",
        json!({
            "operationId":CANCEL_OPERATION,"targetOperationId":PROMPT_OPERATION,
            "target":target,"requestedBy":actor,"approver":actor
        }),
    )
    .await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert_eq!(
        response["result"]["structuredContent"]["operation"]["operationId"],
        CANCEL_OPERATION
    );
    assert_eq!(
        response["result"]["structuredContent"]["operation"]["operation"],
        "conversationCancel"
    );
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

pub(super) fn publish_manifest(
    path: &std::path::Path,
) -> collaboration_service::ManifestPublication {
    collaboration_service::ManifestPublication::publish(
        path,
        &serde_json::from_value(json!({
            "version":2,
            "serviceId":SERVICE_ID,
            "serviceEpoch":SERVICE_EPOCH,
            "control":{"transport":"unixJsonLines","path":"control.sock"},
            "controlSchemaDigest":format!("sha256:{}", "a".repeat(64)),
            "mcp":{"transport":"streamableHttp","url":"http://127.0.0.1:0/mcp"}
        }))
        .expect("service manifest"),
    )
    .expect("manifest publication")
}

pub(super) async fn start_listener(path: &std::path::Path) -> CollaborationMcpListener {
    CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: path.to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts")
}

pub(super) async fn initialize_control(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
) {
    let request = read_json_line(lines).await;
    assert_eq!(request["method"], "control/initialize");
    write_json_line(
        writer,
        json!({"jsonrpc":"2.0","id":request["id"],"result":{"version":{"major":1,"minor":0},"serviceId":SERVICE_ID,"serviceEpoch":SERVICE_EPOCH,"controlSchemaDigest":format!("sha256:{}", "a".repeat(64))}}),
    )
    .await;
}

pub(super) async fn read_json_line(
    lines: &mut tokio::io::Lines<BufReader<tokio::net::unix::OwnedReadHalf>>,
) -> Value {
    serde_json::from_str(
        &lines
            .next_line()
            .await
            .expect("Control read")
            .expect("Control frame"),
    )
    .expect("Control JSON")
}

async fn write_json_line(writer: &mut tokio::net::unix::OwnedWriteHalf, value: Value) {
    writer
        .write_all(format!("{value}\n").as_bytes())
        .await
        .expect("Control write");
}

fn operation_snapshot(operation_id: &str, operation: &str) -> Value {
    json!({
        "operationId":operation_id,
        "operation":operation,
        "binding":{"kind":"externalProvider","binding":{
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
            "bindingId":"binding-1",
            "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp","runtimeVersion":"1.0.0"},
            "transport":"stdioAcp",
            "generation":generation(),
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        }},
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
        "stage":"mayHaveDispatched",
        "effect":"unknown",
        "reconciliation":"unresolved",
        "admittedAt":"2026-09-20T12:00:00Z",
        "terminalAt":null
    })
}

fn terminal_snapshot(operation_id: &str, operation: &str) -> Value {
    let mut snapshot = operation_snapshot(operation_id, operation);
    snapshot["stage"] = json!("terminal");
    snapshot["effect"] = json!("applied");
    snapshot["reconciliation"] = json!("confirmed");
    snapshot["terminalAt"] = json!("2026-09-20T12:00:01Z");
    snapshot
}

fn endpoint_inventory() -> Value {
    json!({
        "serviceEpoch":SERVICE_EPOCH,"sequence":1,
        "endpoints":[{
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
            "label":"Fixture provider",
            "availability":{"state":"available","observedAt":"2026-09-20T12:00:00Z"},
            "channels":[{
                "kind":"externalProvider","transport":"stdioAcp",
                "bindingId":"binding-1","bindingGeneration":3,
                "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp"},
                "capabilities":[{"name":"create","status":"supported","evidence":"advertised"}]
            }]
        }]
    })
}

pub(super) fn generation() -> Value {
    json!({"serviceEpoch":SERVICE_EPOCH,"generation":3})
}

pub(super) async fn initialize_mcp(
    client: &reqwest::Client,
    listener: &CollaborationMcpListener,
) -> reqwest::header::HeaderValue {
    let response = client.post(listener.local_url()).header(CONTENT_TYPE,"application/json").header(ACCEPT,"application/json, text/event-stream").json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"provider-conversation-test","version":"1"}}})).send().await.expect("MCP initialize");
    let session = response
        .headers()
        .get("mcp-session-id")
        .cloned()
        .expect("MCP session");
    let _ = protocol_response_json(response).await;
    let initialized = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
        .send()
        .await
        .expect("MCP initialized");
    assert!(initialized.status().is_success());
    session
}

async fn call_tool(
    client: &reqwest::Client,
    listener: &CollaborationMcpListener,
    session: &reqwest::header::HeaderValue,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    call_mcp(
        client,
        listener,
        session,
        id,
        "tools/call",
        json!({"name":name,"arguments":arguments}),
    )
    .await
}

async fn call_mcp(
    client: &reqwest::Client,
    listener: &CollaborationMcpListener,
    session: &reqwest::header::HeaderValue,
    id: u64,
    method: &str,
    params: Value,
) -> Value {
    let response = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session.clone())
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
        .send()
        .await
        .expect("MCP response");
    protocol_response_json(response).await
}

async fn protocol_response_json(response: reqwest::Response) -> Value {
    let body = response.text().await.expect("MCP body");
    protocol_body_json(&body)
}

fn protocol_body_json(body: &str) -> Value {
    let encoded = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|data| !data.is_empty())
        .unwrap_or(body);
    serde_json::from_str(encoded).unwrap_or_else(|error| panic!("MCP JSON: {error}; body={body:?}"))
}
