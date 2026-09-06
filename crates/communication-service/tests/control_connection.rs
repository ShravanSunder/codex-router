use communication_service::{ServiceIdentity, serve_control_connection};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn actual_unix_connection_initializes_before_discovery() {
    // Arrange: real OS socket pair, no Codex or production endpoint.
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("socket: {error}"));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap_or_else(|error| panic!("identity: {error}"));
    let endpoint = json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Debug Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]});
    let identity = identity
        .with_endpoints(vec![
            serde_json::from_value(endpoint.clone())
                .unwrap_or_else(|error| panic!("endpoint: {error}")),
        ])
        .unwrap_or_else(|error| panic!("inventory: {error}"));
    let task = tokio::spawn(serve_control_connection(server, identity));
    let (reader, mut writer) = client.into_split();
    let mut lines = BufReader::new(reader).lines();
    // Act / Assert
    for (request, expected) in [
        (
            json!({"jsonrpc":"2.0","id":"early","method":"endpoint/list","params":{}}),
            -32600,
        ),
        (
            json!({"jsonrpc":"2.0","id":"init","method":"control/initialize","params":{"version":{"major":1,"minor":0},"client":{"name":"test","version":"1"}}}),
            0,
        ),
        (
            json!({"jsonrpc":"2.0","id":"list","method":"endpoint/list","params":{}}),
            0,
        ),
        (
            json!({"jsonrpc":"2.0","id":"list","method":"endpoint/list","params":{}}),
            -32600,
        ),
    ] {
        let frame = format!("{request}\n");
        writer
            .write_all(frame.as_bytes())
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
        let line = lines
            .next_line()
            .await
            .unwrap_or_else(|error| panic!("read: {error}"))
            .unwrap_or_else(|| panic!("response"));
        let response: Value =
            serde_json::from_str(&line).unwrap_or_else(|error| panic!("JSON: {error}"));
        assert_eq!(response["id"], request["id"]);
        if expected == 0 {
            assert!(response.get("result").is_some());
            if request["method"] == "endpoint/list" {
                assert_eq!(response["result"]["endpoints"], json!([endpoint.clone()]));
            }
        } else {
            assert_eq!(response["error"]["code"], expected);
        }
    }
    writer
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    task.await
        .unwrap_or_else(|error| panic!("join: {error}"))
        .unwrap_or_else(|error| panic!("serve: {error}"));
}
