//! R5 keeps the actual Unix HTTP boundary closed to browser Origins.

use std::os::unix::fs::PermissionsExt as _;
use std::time::Duration;

use collaboration_service::CollaborationApplication;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

use crate::api_test_harness::{ServedApi, TEST_PROTOCOL_VERSION, api_config, test_identity};

#[tokio::test]
async fn unix_mcp_refuses_browser_origins_but_accepts_an_origin_free_client() {
    let directory = tempfile::tempdir_in("/tmp").expect("short Unix socket path");
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
        .expect("owner-only socket directory");
    let socket_path = directory.path().join("control.sock");
    let config = api_config(
        CollaborationApplication::new(test_identity()),
        directory.path(),
    );
    let api = ServedApi::unix(&config, &socket_path).await;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;

    // Even localhost and the prospective private HTTPS origin are browser Origins.
    for origin in [
        "https://invalid.example",
        "http://localhost",
        "https://node.example.ts.net:8443",
    ] {
        let response_status = tokio::time::timeout(Duration::from_secs(5), async {
            let mut stream = tokio::net::UnixStream::connect(&socket_path)
                .await
                .expect("connect to the actual Unix listener");
            let request = format!(
                "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {TEST_PROTOCOL_VERSION}\r\nOrigin: {origin}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(request.as_bytes())
                .await
                .expect("write HTTP request");
            stream.flush().await.expect("flush HTTP request");
            let mut response_status = String::new();
            BufReader::new(stream)
                .read_line(&mut response_status)
                .await
                .expect("read HTTP status");
            response_status
        })
        .await
        .expect("Unix Origin rejection completes within its protocol bound");

        assert_eq!(response_status, "HTTP/1.1 403 Forbidden\r\n", "{origin}");
    }

    // The positive control uses a real rmcp handshake and inventory on the same listener.
    let client = tokio::time::timeout(Duration::from_secs(5), api.client())
        .await
        .expect("Origin-free Unix client connects");
    let tools = tokio::time::timeout(Duration::from_secs(5), client.list_all_tools())
        .await
        .expect("Origin-free inventory completes")
        .expect("Origin-free inventory succeeds");
    assert!(tools.iter().any(|tool| tool.name == "board_project_list"));
    client
        .cancel()
        .await
        .expect("close positive-control client");
    api.stop().await;
}
