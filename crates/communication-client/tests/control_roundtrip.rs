use communication_client::ControlClient;
use communication_service::{ServiceIdentity, serve_control_connection};

#[tokio::test]
async fn client_initializes_and_discovers_over_real_unix_transport() {
    // Arrange: no backend process; only the owned public service boundary.
    let (client, server) =
        tokio::net::UnixStream::pair().unwrap_or_else(|error| panic!("pair: {error}"));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap_or_else(|error| panic!("identity: {error}"));
    let directory = identity.endpoint_directory();
    let task = tokio::spawn(serve_control_connection(server, identity));
    // Act
    let mut client = ControlClient::initialize(client, "client-proof", "1")
        .await
        .unwrap_or_else(|error| panic!("initialize: {error}"));
    let inventory = client
        .list_endpoints()
        .await
        .unwrap_or_else(|error| panic!("inventory: {error}"));
    // Assert
    assert!(inventory.endpoints.is_empty());
    assert_eq!(inventory.service_epoch, client.identity().service_epoch);
    let endpoint = serde_json::json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"Debug Codex","availability":{"state":"unprobed"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"codex-native.sock","schemaDigest":null,"generation":null}]});
    directory
        .publish(
            serde_json::from_value(endpoint.clone())
                .unwrap_or_else(|error| panic!("endpoint: {error}")),
        )
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let event = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        client.next_notification(),
    )
    .await
    .unwrap_or_else(|error| panic!("event timeout: {error}"))
    .unwrap_or_else(|error| panic!("event: {error}"));
    assert_eq!(event["params"]["endpoint"], endpoint);
    let current = client
        .list_endpoints()
        .await
        .unwrap_or_else(|error| panic!("updated inventory: {error}"));
    assert_eq!(current.sequence, 1);
    assert_eq!(current.endpoints.len(), 1);
    client
        .close()
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    task.await
        .unwrap_or_else(|error| panic!("join: {error}"))
        .unwrap_or_else(|error| panic!("server: {error}"));
}
