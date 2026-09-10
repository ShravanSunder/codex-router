use codex_router_host::{CommunicationRuntime, CommunicationRuntimeInputs};
use communication_client::{ControlClient, JournalStatus};
use communication_protocol::EndpointAvailability;
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn journal_open_failure_does_not_disable_control_and_native_publication() {
    // Arrange: an owner-private fixture path deliberately cannot be opened as SQLite.
    let root = std::env::temp_dir().join(format!("journal-isolation-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let blocked_database = root.join("session-registry.sqlite");
    std::fs::create_dir(&blocked_database)
        .unwrap_or_else(|error| panic!("blocked database: {error}"));
    // Act: callers can still discover the service and its explicit storage-unavailable state.
    let mut runtime = CommunicationRuntime::start(CommunicationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("backend.sock"),
        native_schema: None,
    })
    .await
    .unwrap_or_else(|error| panic!("communication startup: {error}"));
    let mut client = ControlClient::connect(&root, "journal-failure-proof", "1")
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    let control_digest = String::from(client.identity().control_schema_digest.clone());
    let control_schema_path = root.join(format!(
        "control-schema-{}.json",
        control_digest.trim_start_matches("sha256:")
    ));
    let status = client
        .journal_status()
        .await
        .unwrap_or_else(|error| panic!("status: {error}"));
    runtime
        .backend_ready(
            "2026-09-05T12:00:00Z"
                .to_owned()
                .try_into()
                .unwrap_or_else(|error| panic!("time: {error}")),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("fixture readiness: {error}"));
    let endpoints = client
        .list_endpoints()
        .await
        .unwrap_or_else(|error| panic!("endpoints: {error}"));
    let listener_present = root.join("codex-native.sock").exists();
    client
        .close()
        .await
        .unwrap_or_else(|error| panic!("close: {error}"));
    runtime
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
    std::fs::remove_file(control_schema_path)
        .unwrap_or_else(|error| panic!("Control schema cleanup: {error}"));
    std::fs::remove_file(root.join("service-identity.json"))
        .unwrap_or_else(|error| panic!("identity cleanup: {error}"));
    std::fs::remove_dir(blocked_database)
        .unwrap_or_else(|error| panic!("database directory cleanup: {error}"));
    std::fs::remove_file(root.join("automation.sqlite"))
        .unwrap_or_else(|error| panic!("automation database cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
    // Assert: this proves listener/publication independence, not a running Codex backend.
    assert!(matches!(status, JournalStatus::Unavailable));
    assert!(listener_present);
    assert!(matches!(
        endpoints
            .endpoints
            .first()
            .map(|endpoint| &endpoint.availability),
        Some(EndpointAvailability::Available { .. })
    ));
}
