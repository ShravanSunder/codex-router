//! Caller cancellation detaches from conversation operations without provider cancellation.
use super::provider_conversation_http_tests::{
    CREATE_OPERATION, SERVICE_ID, WAIT_OPERATION, generation, initialize_control, initialize_mcp,
    publish_manifest, read_json_line, start_listener,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use rmcp::{ServerHandler as _, ServiceExt as _};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};

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
            .json(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"conversation_operation_wait","arguments":{"operationId":WAIT_OPERATION,"timeoutSeconds":60}}}))
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
            .json(&json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"conversation_create","arguments":{
                "operationId":CREATE_OPERATION,
                "endpoint":endpoint,
                "generation":generation(),
                "workingDirectory":working_directory,
                "createdBy":actor,
                "approver":actor,
                "access":"workspace-write"
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
        "access":"workspace-write"
    });
    let mut call = Box::pin(
        handler.call_tool(
            rmcp::model::CallToolRequestParams::new("conversation_create")
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
