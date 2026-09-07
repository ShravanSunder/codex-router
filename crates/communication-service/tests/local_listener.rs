use communication_client::ControlClient;
use communication_service::{LocalControlService, ServiceIdentity};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn owned_listener_is_private_and_shutdown_removes_only_its_socket() {
    let directory =
        std::path::PathBuf::from(format!("/tmp/control-listener-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap_or_else(|e| panic!("directory: {e}"));
    let socket = directory.join("control.sock");
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap_or_else(|e| panic!("identity: {e}"));
    let service = LocalControlService::bind(&socket, identity.clone())
        .unwrap_or_else(|e| panic!("bind: {e}"));
    assert_eq!(
        std::fs::metadata(&socket)
            .unwrap_or_else(|e| panic!("metadata: {e}"))
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(LocalControlService::bind(&socket, identity).is_err());
    let stop = CancellationToken::new();
    let task = tokio::spawn(service.run(stop.clone()));
    let stream = tokio::net::UnixStream::connect(&socket)
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));
    let mut client = ControlClient::initialize(stream, "listener-proof", "1")
        .await
        .unwrap_or_else(|e| panic!("initialize: {e}"));
    assert!(
        client
            .list_endpoints()
            .await
            .unwrap_or_else(|e| panic!("list: {e}"))
            .endpoints
            .is_empty()
    );
    stop.cancel();
    task.await
        .unwrap_or_else(|e| panic!("join: {e}"))
        .unwrap_or_else(|e| panic!("service: {e}"));
    assert!(!socket.exists());
    std::fs::remove_dir(directory).unwrap_or_else(|e| panic!("cleanup: {e}"));
}

#[tokio::test]
async fn dropping_old_listener_preserves_replacement_at_same_path() {
    // Arrange
    let directory =
        std::path::PathBuf::from(format!("/tmp/control-replacement-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let socket = directory.join("control.sock");
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap_or_else(|error| panic!("identity: {error}"));
    let original = LocalControlService::bind(&socket, identity)
        .unwrap_or_else(|error| panic!("bind: {error}"));
    // Act: replace only this test-owned socket path while the original handle remains alive.
    std::fs::remove_file(&socket).unwrap_or_else(|error| panic!("unlink fixture: {error}"));
    let replacement = tokio::net::UnixListener::bind(&socket)
        .unwrap_or_else(|error| panic!("replacement: {error}"));
    drop(original);
    // Assert
    assert!(socket.exists());
    assert!(tokio::net::UnixStream::connect(&socket).await.is_ok());
    drop(replacement);
    std::fs::remove_file(&socket).unwrap_or_else(|error| panic!("cleanup socket: {error}"));
    std::fs::remove_dir(directory).unwrap_or_else(|error| panic!("cleanup directory: {error}"));
}
