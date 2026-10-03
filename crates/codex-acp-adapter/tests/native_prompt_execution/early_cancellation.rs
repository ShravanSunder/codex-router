use super::*;
use std::error::Error;

type TestResult = Result<(), Box<dyn Error + Send + Sync>>;

#[derive(Clone, Copy)]
enum CancellationPoint {
    BeforeDispatch,
    DuringNativeStart,
}

#[tokio::test]
async fn prompt_actor_cancellation_before_dispatch_includes_null_final_reply() -> TestResult {
    assert_early_cancellation(CancellationPoint::BeforeDispatch).await
}

#[tokio::test]
async fn prompt_actor_cancellation_during_native_startup_includes_null_final_reply() -> TestResult {
    assert_early_cancellation(CancellationPoint::DuringNativeStart).await
}

#[allow(clippy::panic_in_result_fn)]
async fn assert_early_cancellation(cancellation_point: CancellationPoint) -> TestResult {
    ensure_test_scratch();
    let mut catalog = AcpSchemaCatalog::load()?;
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
    let schema_document = json!({"definitions":{
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
    }});
    let bundle = NativeSchemaBundle::from_documents(BTreeMap::from([(
        "codex_app_server_protocol.schemas.json".to_owned(),
        serde_json::to_vec(&schema_document)?,
    )]))?;
    let schemas = Arc::new(NativePayloadSchemas::from_bundle(&bundle)?);
    let generation = serde_json::from_value(
        json!({"serviceEpoch":"00000000-0000-4000-8000-000000000001","generation":1}),
    )?;
    let (client, server) = tokio::net::UnixStream::pair()?;
    let connection = NativeProtocolConnection::from_websocket(
        WebSocketStream::from_raw_socket(client, Role::Client, None).await,
    );
    let (turn_start_sender, turn_start_receiver) = tokio::sync::oneshot::channel();
    let fixture = tokio::spawn(run_native_fixture(
        server,
        cancellation_point,
        turn_start_sender,
    ));
    let setup_inputs = SessionSetupInputs {
        operation_id: None,
        recorder: Arc::new(AcceptingConversationRecorder),
        connection,
        schemas,
        generation,
        params: json!({"cwd":"/work","mcpServers":[],"_meta":{"codexRouter":{"model":"gpt-5.6-sol","effort":"medium","access":"workspace-write","scratchScope":"session-00000000-0000-4000-8000-000000000099","scratchPath":TEST_SCRATCH,"createdBy":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"},"approver":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"sessionId":"creator"}}}}),
        approval_broker: Arc::new(codex_acp_adapter::RejectingApprovalBroker),
    };
    let session = AcpSessionBinding::create(&mut catalog, setup_inputs).await?;
    let retired = tokio_util::sync::CancellationToken::new();
    let (output, mut frames) = codex_acp_adapter::bounded_acp_output(retired.clone());
    let mut registry =
        codex_acp_adapter::AcpSessionRegistry::new(output, retired, Arc::new(TestBindingHolder));
    registry
        .insert(session)
        .map_err(|(error, _session)| error)?;
    let params = json!({"sessionId":"thread-a","prompt":[{"type":"text","text":"hello"}],"_meta":{"codexRouter":{"effort":"medium"}}});
    registry.begin_prompt(&mut catalog, json!("acp-prompt"), params)?;

    let expected_interruption = match cancellation_point {
        CancellationPoint::BeforeDispatch => "notDispatched",
        CancellationPoint::DuringNativeStart => {
            tokio::time::timeout(std::time::Duration::from_secs(3), turn_start_receiver).await??;
            "unknown"
        }
    };
    registry.cancel("thread-a")?;
    tokio::time::timeout(std::time::Duration::from_secs(3), registry.complete_next()).await??;
    let terminal = frames
        .recv()
        .await
        .ok_or_else(|| std::io::Error::other("cancel response was not published"))?;
    assert_eq!(
        terminal
            .pointer("/result/stopReason")
            .and_then(Value::as_str),
        Some("cancelled")
    );
    assert_eq!(
        terminal
            .pointer("/result/_meta/codex-router~1nativeInterruption/state")
            .and_then(Value::as_str),
        Some(expected_interruption)
    );
    assert_eq!(
        terminal.pointer("/result/_meta/codex-router~1finalReply"),
        Some(&json!({"kind":"available","text":null}))
    );
    registry.shutdown().await;
    fixture.await??;
    Ok(())
}

async fn run_native_fixture(
    server: tokio::net::UnixStream,
    cancellation_point: CancellationPoint,
    turn_start_sender: tokio::sync::oneshot::Sender<()>,
) -> TestResult {
    let mut socket = WebSocketStream::from_raw_socket(server, Role::Server, None).await;
    let start_request = receive_request(&mut socket, "thread/start").await?;
    let start_request_id = start_request
        .get("id")
        .ok_or_else(|| std::io::Error::other("thread/start request ID is missing"))?;
    socket
        .send(Message::Text(
            json!({"id":start_request_id,"result":{"cwd":"/work","model":"gpt-5.6-sol","approvalPolicy":"on-request","approvalsReviewer":"auto_review","activePermissionProfile":{"id":"router-workspace-write","extends":":workspace"},"sandbox":applied_router_sandbox(&start_request),"thread":{"id":"thread-a","cwd":"/work","status":{"type":"idle"},"reasoningEffort":"medium","turns":[]}}})
                .to_string()
                .into(),
        ))
        .await?;

    if matches!(cancellation_point, CancellationPoint::BeforeDispatch) {
        let _ = socket.next().await;
        return Ok(());
    }

    let thread_read = receive_request(&mut socket, "thread/read").await?;
    let thread_read_id = thread_read
        .get("id")
        .ok_or_else(|| std::io::Error::other("thread/read request ID is missing"))?;
    socket
        .send(Message::Text(
            json!({"id":thread_read_id,"result":{"thread":{"id":"thread-a","status":{"type":"idle"},"turns":[]}}})
                .to_string()
                .into(),
        ))
        .await?;
    let turn_start = receive_request(&mut socket, "turn/start").await?;
    assert_eq!(
        turn_start
            .pointer("/params/input/0/text")
            .and_then(Value::as_str),
        Some("hello")
    );
    let _ = turn_start_sender.send(());
    let _ = socket.next().await;
    Ok(())
}

#[allow(clippy::panic_in_result_fn)]
async fn receive_request(
    socket: &mut WebSocketStream<tokio::net::UnixStream>,
    expected_method: &str,
) -> Result<Value, Box<dyn Error + Send + Sync>> {
    let frame = socket
        .next()
        .await
        .ok_or_else(|| std::io::Error::other(format!("request for {expected_method}")))??;
    let request: Value = serde_json::from_str(frame.to_text()?)?;
    assert_eq!(
        request.get("method").and_then(Value::as_str),
        Some(expected_method)
    );
    Ok(request)
}
