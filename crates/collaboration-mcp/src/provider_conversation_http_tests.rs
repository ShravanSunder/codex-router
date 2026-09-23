use super::{CollaborationMcpListener, CollaborationMcpListenerConfig, LoopbackBindAddress};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use rmcp::{ServerHandler as _, ServiceExt as _};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SERVICE_ID: &str = "00000000-0000-4000-8000-000000000001";
const SERVICE_EPOCH: &str = "00000000-0000-4000-8000-000000000002";
const CREATE_OPERATION: &str = "019f0000-0000-7000-8000-000000000001";
const PROMPT_OPERATION: &str = "019f0000-0000-7000-8000-000000000002";
const WAIT_OPERATION: &str = "019f0000-0000-7000-8000-000000000003";
const LOAD_OPERATION: &str = "019f0000-0000-7000-8000-000000000004";

#[tokio::test]
async fn initialized_http_exposes_and_calls_provider_conversation_tools() {
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
            let snapshot = operation_snapshot(operation_id, operation);
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
        }
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;

    let tools = call_mcp(&client, &listener, &session, 2, "tools/list", json!({})).await;
    let listed = tools["result"]["tools"].as_array().expect("tool array");
    for name in [
        "provider_conversation_create",
        "provider_conversation_load",
        "provider_conversation_prompt",
        "provider_conversation_cancel",
        "provider_conversation_operation_show",
        "provider_conversation_operation_wait",
        "provider_conversation_operation_reconcile",
    ] {
        let tool = listed
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("missing provider tool {name}"));
        if matches!(
            name,
            "provider_conversation_create"
                | "provider_conversation_load"
                | "provider_conversation_prompt"
                | "provider_conversation_cancel"
        ) {
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
        "provider_conversation_create",
        json!({
            "operationId":CREATE_OPERATION,
            "endpoint":endpoint,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "createdBy":actor,
            "approver":actor,
            "requestedPolicy":{"access":"workspace-write"}
        }),
    )
    .await;
    assert_eq!(create["result"]["isError"], false);
    assert_eq!(
        create["result"]["structuredContent"]["operation"]["operationId"],
        CREATE_OPERATION
    );

    let target = json!({"endpoint":endpoint,"sessionId":"provider-thread"});
    let prompt = call_tool(
        &client,
        &listener,
        &session,
        4,
        "provider_conversation_prompt",
        json!({
            "operationId":PROMPT_OPERATION,
            "target":target,
            "generation":generation(),
            "requestedBy":actor,
            "approver":actor,
            "prompt":{"kind":"humanUser","text":"continue the assignment"}
        }),
    )
    .await;
    assert_eq!(prompt["result"]["isError"], false);
    assert_eq!(
        prompt["result"]["structuredContent"]["operation"]["operationId"],
        PROMPT_OPERATION
    );

    let show = call_tool(
        &client,
        &listener,
        &session,
        5,
        "provider_conversation_operation_show",
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
        "provider_conversation_load",
        json!({
            "operationId":LOAD_OPERATION,
            "target":target,
            "generation":generation(),
            "workingDirectory":temporary.path(),
            "requestedBy":actor,
            "approver":actor,
            "requestedPolicy":{"access":"workspace-write"}
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
async fn cancelling_provider_operation_wait_detaches_without_backend_cancel() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let (wait_started_tx, wait_started_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        initialize_control(&mut lines, &mut writer).await;
        let request = read_json_line(&mut lines).await;
        assert_eq!(request["method"], "conversation/operationWait");
        assert_eq!(request["params"]["operationId"], WAIT_OPERATION);
        wait_started_tx.send(()).expect("wait started signal");
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                .await
                .expect("call-local Control connection closes")
                .expect("Control EOF read")
                .is_none(),
            "MCP cancellation must detach the waiter without sending conversation/cancel"
        );
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;
    let request_client = client.clone();
    let request_url = listener.local_url();
    let request_session = session.clone();
    let mut wait_request = tokio::spawn(async move {
        let response = request_client
            .post(request_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", request_session)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"provider_conversation_operation_wait","arguments":{"operationId":WAIT_OPERATION,"timeoutSeconds":60}}}))
            .send()
            .await?;
        response.bytes().await
    });
    tokio::select! {
        started = wait_started_rx => started.expect("provider wait started"),
        result = &mut wait_request => panic!("wait ended before cancellation: {result:?}"),
    }
    let cancellation = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9,"reason":"caller stopped waiting"}}))
        .send()
        .await
        .expect("MCP cancellation response");
    assert!(cancellation.status().is_success());
    let _request_result = tokio::time::timeout(std::time::Duration::from_secs(1), wait_request)
        .await
        .expect("cancelled MCP wait settles")
        .expect("wait request join");
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn cancellation_during_control_connect_is_no_effect_for_mutation() {
    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let (initialize_started_tx, initialize_started_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, _writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize = read_json_line(&mut lines).await;
        assert_eq!(initialize["method"], "control/initialize");
        initialize_started_tx
            .send(())
            .expect("initialize started signal");
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), lines.next_line())
                .await
                .expect("call-local Control connection closes")
                .expect("Control EOF read")
                .is_none(),
            "cancellation before Control initialization must not submit the mutation"
        );
    });
    let listener = start_listener(temporary.path()).await;
    let client = reqwest::Client::new();
    let session = initialize_mcp(&client, &listener).await;
    let request_client = client.clone();
    let request_url = listener.local_url();
    let request_session = session.clone();
    let working_directory = temporary.path().to_owned();
    let mut create_request = tokio::spawn(async move {
        let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
        let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
        let response = request_client
            .post(request_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("mcp-session-id", request_session)
            .header("mcp-protocol-version", "2025-11-25")
            .json(&json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"provider_conversation_create","arguments":{
                "operationId":CREATE_OPERATION,
                "endpoint":endpoint,
                "generation":generation(),
                "workingDirectory":working_directory,
                "createdBy":actor,
                "approver":actor,
                "requestedPolicy":{"access":"workspace-write"}
            }}}))
            .send()
            .await?;
        response.text().await
    });
    tokio::select! {
        started = initialize_started_rx => started.expect("Control initialize started"),
        result = &mut create_request => panic!("create ended before cancellation: {result:?}"),
    }
    let cancellation = client
        .post(listener.local_url())
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("mcp-protocol-version", "2025-11-25")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":10,"reason":"caller cancelled before connect"}}))
        .send()
        .await
        .expect("MCP cancellation response");
    assert!(cancellation.status().is_success());
    let body = tokio::time::timeout(std::time::Duration::from_secs(1), create_request)
        .await
        .expect("cancelled MCP create settles")
        .expect("create request join")
        .expect("create response body");
    assert!(
        !body.contains(CREATE_OPERATION),
        "cancelled stream must not fabricate a mutation result"
    );
    peer.await.expect("Control peer");
    listener.shutdown().await.expect("listener shutdown");
}

#[tokio::test]
async fn registered_mutation_handler_returns_no_effect_when_connect_is_cancelled() {
    #[derive(Clone)]
    struct TestClient;
    impl rmcp::handler::client::ClientHandler for TestClient {}

    let temporary = tempfile::tempdir().expect("temporary service directory");
    let _publication = publish_manifest(temporary.path());
    let control = tokio::net::UnixListener::bind(temporary.path().join("control.sock"))
        .expect("Control listener");
    let (initialize_started_tx, initialize_started_rx) = tokio::sync::oneshot::channel();
    let peer = tokio::spawn(async move {
        let (stream, _) = control.accept().await.expect("Control accept");
        let (reader, _writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        let initialize = read_json_line(&mut lines).await;
        assert_eq!(initialize["method"], "control/initialize");
        initialize_started_tx
            .send(())
            .expect("initialize started signal");
        assert!(
            lines.next_line().await.expect("Control EOF read").is_none(),
            "cancelled handler must not submit the mutation"
        );
    });
    let server = crate::mcp_server::CollaborationMcpServer::new(temporary.path().to_owned());
    let handler = server.clone();
    let (server_transport, client_transport) = tokio::io::duplex(4096);
    let (running_server, running_client) = tokio::join!(
        server.serve(server_transport),
        TestClient.serve(client_transport),
    );
    let running_server = running_server.expect("test MCP server");
    let running_client = running_client.expect("test MCP client");
    let request_context = rmcp::service::RequestContext::new(
        rmcp::model::NumberOrString::Number(10),
        running_server.peer().clone(),
    );
    let cancellation = request_context.ct.clone();
    let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
    let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
    let arguments = json!({
        "operationId":CREATE_OPERATION,
        "endpoint":endpoint,
        "generation":generation(),
        "workingDirectory":temporary.path(),
        "createdBy":actor,
        "approver":actor,
        "requestedPolicy":{"access":"workspace-write"}
    });
    let mut call = Box::pin(
        handler.call_tool(
            rmcp::model::CallToolRequestParams::new("provider_conversation_create")
                .with_arguments(arguments.as_object().expect("arguments").clone()),
            request_context,
        ),
    );
    tokio::select! {
        started = initialize_started_rx => started.expect("Control initialize started"),
        result = &mut call => panic!("registered handler ended before cancellation: {result:?}"),
    }
    cancellation.cancel();
    let response = call.await.expect("registered handler response");
    let rmcp::model::CallToolResponse::Complete(result) = response else {
        panic!("expected complete tool result")
    };
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured cancellation");
    assert_eq!(structured["kind"], "callerCancelled");
    assert_eq!(structured["effect"], "none");
    peer.await.expect("Control peer");
    running_client.cancel().await.expect("client cancel");
    running_server.cancel().await.expect("server cancel");
}

fn publish_manifest(path: &std::path::Path) -> collaboration_service::ManifestPublication {
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

async fn start_listener(path: &std::path::Path) -> CollaborationMcpListener {
    CollaborationMcpListener::start(CollaborationMcpListenerConfig {
        bind_address: LoopbackBindAddress::parse("127.0.0.1:0").expect("loopback bind"),
        service_directory: path.to_owned(),
        allowed_origins: Vec::new(),
    })
    .await
    .expect("MCP listener starts")
}

async fn initialize_control(
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

async fn read_json_line(
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
        "binding":{
            "endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},
            "bindingId":"binding-1",
            "runtime":{"provider":"claudeCode","runtimeName":"claude-agent-acp","runtimeVersion":"1.0.0"},
            "transport":"stdioAcp",
            "generation":generation(),
            "capabilities":[{"name":"prompt","status":"supported","evidence":"advertised"}]
        },
        "target":{"endpoint":{"serviceId":SERVICE_ID,"endpointId":"claude-code"},"sessionId":"provider-thread"},
        "stage":"mayHaveDispatched",
        "effect":"unknown",
        "reconciliation":"unresolved",
        "admittedAt":"2026-09-20T12:00:00Z",
        "terminalAt":null
    })
}

fn generation() -> Value {
    json!({"serviceEpoch":SERVICE_EPOCH,"generation":3})
}

async fn initialize_mcp(
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
