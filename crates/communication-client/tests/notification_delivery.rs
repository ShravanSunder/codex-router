use communication_client::{ClientError, ControlClient};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn notifications_before_response_and_while_idle_are_received() {
    let (client, server) = tokio::net::UnixStream::pair().unwrap_or_else(|e| panic!("pair: {e}"));
    let task = tokio::spawn(async move {
        let (read, mut write) = server.into_split();
        let line = BufReader::new(read)
            .lines()
            .next_line()
            .await
            .unwrap_or_else(|e| panic!("read: {e}"))
            .unwrap_or_else(|| panic!("request"));
        let request: Value = serde_json::from_str(&line).unwrap_or_else(|e| panic!("JSON: {e}"));
        let epoch = "00000000-0000-4000-8000-000000000002";
        let event = |sequence| json!({"jsonrpc":"2.0","method":"endpoint/changed","params":{"serviceEpoch":epoch,"sequence":sequence,"endpoint":{"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]}}});
        let response = json!({"jsonrpc":"2.0","id":request["id"],"result":{"version":{"major":1,"minor":0},"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":epoch,"controlSchemaDigest":format!("sha256:{}","a".repeat(64))}});
        let frames = format!("{}\n{}\n{}\n", event(1), response, event(2));
        write
            .write_all(frames.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("write: {e}"));
    });
    let mut client = ControlClient::initialize(client, "test", "1")
        .await
        .unwrap_or_else(|e| panic!("initialize: {e}"));
    for sequence in [1, 2] {
        let event = client
            .next_notification()
            .await
            .unwrap_or_else(|e| panic!("notification: {e}"));
        assert_eq!(event["params"]["sequence"], sequence);
    }
    assert!(matches!(
        client.next_notification().await,
        Err(ClientError::Protocol(_))
    ));
    task.await.unwrap_or_else(|e| panic!("join: {e}"));
}

#[tokio::test]
async fn mismatched_response_is_rejected_without_replaying_initialization() {
    // Arrange
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let task = tokio::spawn(async move {
        let (read, mut write) = server.into_split();
        let mut lines = BufReader::new(read).lines();
        assert!(
            lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("read: {error}"))
                .is_some()
        );
        write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"another-request\",\"result\":{}}\n")
            .await
            .unwrap_or_else(|error| panic!("write: {error}"));
        // The failed initialization closes its connection, rather than resending.
        assert!(
            lines
                .next_line()
                .await
                .unwrap_or_else(|error| panic!("EOF: {error}"))
                .is_none()
        );
    });
    // Act / Assert
    assert!(matches!(
        ControlClient::initialize(client, "test", "1").await,
        Err(ClientError::Protocol("response ID mismatch"))
    ));
    task.await.unwrap_or_else(|error| panic!("join: {error}"));
}
