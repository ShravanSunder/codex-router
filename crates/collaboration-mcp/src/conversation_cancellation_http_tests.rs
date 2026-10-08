//! A caller that goes away detaches from conversation operations without provider cancellation.
use super::provider_conversation_http_tests::{
    CREATE_OPERATION, SERVICE_ID, generation, publish_manifest, read_json_line, start_listener,
};
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use rmcp::{ServerHandler as _, ServiceExt as _};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::test]
async fn a_caller_that_disconnects_during_control_connect_submits_no_mutation() {
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
    let request_url = listener.url();
    let working_directory = temporary.path().to_owned();
    let mut create_request = tokio::spawn(async move {
        let endpoint = json!({"serviceId":SERVICE_ID,"endpointId":"claude-code"});
        let actor = json!({"endpoint":endpoint,"sessionId":"caller-session"});
        let response = reqwest::Client::new()
            .post(request_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
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
    // The caller goes away: its HTTP connection closes with the call in flight.
    create_request.abort();
    let _aborted = create_request.await;
    peer.await.expect("Control peer");
    listener
        .await_active_calls(0, std::time::Duration::from_secs(5))
        .await;
    listener.stop().await;
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
    let server = crate::mcp_server::CollaborationMcpServer::for_application(
        collaboration_service::CollaborationApplication::new(
            crate::api_test_harness::test_identity(),
        ),
        temporary.path().to_owned(),
    );
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
