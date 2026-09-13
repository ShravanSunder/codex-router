use collaboration_client::{NativeTransportConnection, NativeTransportError, protocol::EndpointId};
use collaboration_service::{LocalControlService, ManifestPublication, ServiceIdentity};
use futures_util::{SinkExt, StreamExt};
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn client_opens_advertised_native_websocket_without_protocol_initialization() {
    let root = std::path::PathBuf::from(format!("/tmp/native-client-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let digest = format!("sha256:{}", "a".repeat(64));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000002",
        &digest,
    )
    .unwrap_or_else(|error| panic!("identity: {error}"));
    let native_path = root.join("native.sock");
    let native_listener = tokio::net::UnixListener::bind(&native_path)
        .unwrap_or_else(|error| panic!("native bind: {error}"));
    let native_peer = tokio::spawn(async move {
        let (stream, _) = native_listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("accept: {error}"));
        let mut socket = tokio_tungstenite::accept_async(stream)
            .await
            .unwrap_or_else(|error| panic!("upgrade: {error}"));
        let first =
            tokio::time::timeout(std::time::Duration::from_millis(100), socket.next()).await;
        assert!(
            first.is_err(),
            "client sent native initialization or replay"
        );
        socket
            .send(tokio_tungstenite::tungstenite::Message::Text(
                "{\"method\":\"fixture/ready\"}".into(),
            ))
            .await
            .unwrap_or_else(|error| panic!("send: {error}"));
    });
    let endpoint=serde_json::from_value(serde_json::json!({"endpoint":{"serviceId":"00000000-0000-4000-8000-000000000001","endpointId":"codex-local"},"label":"fixture","availability":{"state":"available","observedAt":"2026-09-13T12:00:00Z"},"channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]})).unwrap_or_else(|error|panic!("endpoint: {error}"));
    let identity = identity
        .with_endpoints(vec![endpoint])
        .unwrap_or_else(|error| panic!("register: {error}"));
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let manifest=serde_json::from_value(serde_json::json!({"version":1,"serviceId":"00000000-0000-4000-8000-000000000001","serviceEpoch":"00000000-0000-4000-8000-000000000002","control":{"transport":"unixJsonLines","path":"control.sock"},"controlSchemaDigest":digest})).unwrap_or_else(|error|panic!("manifest: {error}"));
    let publication = ManifestPublication::publish(&root, &manifest)
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let stop = tokio_util::sync::CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));

    let endpoint_id = EndpointId::try_from("codex-local".to_owned())
        .unwrap_or_else(|error| panic!("endpoint ID: {error}"));
    let mut connection = NativeTransportConnection::connect(&root, endpoint_id)
        .await
        .unwrap_or_else(|error| panic!("connect: {error}"));
    assert_eq!(
        String::from(connection.endpoint.endpoint_id.clone()),
        "codex-local"
    );
    let message = connection
        .stream
        .next()
        .await
        .unwrap_or_else(|| panic!("native socket ended"))
        .unwrap_or_else(|error| panic!("native frame: {error}"));
    assert_eq!(
        message.to_text().ok(),
        Some("{\"method\":\"fixture/ready\"}")
    );

    native_peer
        .await
        .unwrap_or_else(|error| panic!("native peer: {error}"));
    drop(connection);
    stop.cancel();
    service
        .await
        .unwrap_or_else(|error| panic!("service join: {error}"))
        .unwrap_or_else(|error| panic!("service: {error}"));
    drop(publication);
    std::fs::remove_file(native_path).unwrap_or_else(|error| panic!("native cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("directory cleanup: {error}"));
}

#[tokio::test]
async fn client_rejects_unavailable_and_escaped_native_endpoints_without_connecting() {
    let suffix = std::process::id();
    let root = std::path::PathBuf::from(format!("/tmp/native-client-reject-{suffix}"));
    let outside = std::path::PathBuf::from(format!("/tmp/native-client-outside-{suffix}"));
    for directory in [&root, &outside] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .unwrap_or_else(|error| panic!("directory: {error}"));
    }
    let digest = format!("sha256:{}", "b".repeat(64));
    let identity = ServiceIdentity::new(
        "00000000-0000-4000-8000-000000000011",
        "00000000-0000-4000-8000-000000000012",
        &digest,
    )
    .unwrap_or_else(|error| panic!("identity: {error}"));
    let endpoint_directory = identity.endpoint_directory();
    let unavailable = serde_json::from_value(serde_json::json!({
        "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000011","endpointId":"codex-local"},
        "label":"fixture",
        "availability":{"state":"unavailable","observedAt":"2026-09-13T12:00:00Z","reason":"fixture offline"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]
    }))
    .unwrap_or_else(|error| panic!("endpoint: {error}"));
    let identity = identity
        .with_endpoints(vec![unavailable])
        .unwrap_or_else(|error| panic!("register: {error}"));
    let listener = LocalControlService::bind(&root.join("control.sock"), identity)
        .unwrap_or_else(|error| panic!("bind: {error}"));
    let manifest = serde_json::from_value(serde_json::json!({
        "version":1,
        "serviceId":"00000000-0000-4000-8000-000000000011",
        "serviceEpoch":"00000000-0000-4000-8000-000000000012",
        "control":{"transport":"unixJsonLines","path":"control.sock"},
        "controlSchemaDigest":digest
    }))
    .unwrap_or_else(|error| panic!("manifest: {error}"));
    let publication = ManifestPublication::publish(&root, &manifest)
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let stop = tokio_util::sync::CancellationToken::new();
    let service = tokio::spawn(listener.run(stop.clone()));
    let endpoint_id = || {
        EndpointId::try_from("codex-local".to_owned())
            .unwrap_or_else(|error| panic!("endpoint ID: {error}"))
    };

    let unavailable_error = NativeTransportConnection::connect(&root, endpoint_id())
        .await
        .err()
        .unwrap_or_else(|| panic!("unavailable endpoint unexpectedly connected"));
    assert!(matches!(
        unavailable_error,
        NativeTransportError::EndpointUnavailable
    ));

    let outside_socket = outside.join("native.sock");
    let native_listener = tokio::net::UnixListener::bind(&outside_socket)
        .unwrap_or_else(|error| panic!("native bind: {error}"));
    std::os::unix::fs::symlink(&outside_socket, root.join("native.sock"))
        .unwrap_or_else(|error| panic!("native symlink: {error}"));
    endpoint_directory
        .publish(
            serde_json::from_value(serde_json::json!({
                "endpoint":{"serviceId":"00000000-0000-4000-8000-000000000011","endpointId":"codex-local"},
                "label":"fixture",
                "availability":{"state":"available","observedAt":"2026-09-13T12:01:00Z"},
                "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock","schemaDigest":null,"generation":null}]
            }))
            .unwrap_or_else(|error| panic!("endpoint: {error}")),
        )
        .unwrap_or_else(|error| panic!("publish: {error}"));
    let escaped_error = NativeTransportConnection::connect(&root, endpoint_id())
        .await
        .err()
        .unwrap_or_else(|| panic!("escaped endpoint unexpectedly connected"));
    assert!(matches!(
        escaped_error,
        NativeTransportError::ChannelEscapesServiceDirectory
    ));
    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            native_listener.accept()
        )
        .await
        .is_err(),
        "escaped native listener accepted a connection"
    );

    stop.cancel();
    service
        .await
        .unwrap_or_else(|error| panic!("service join: {error}"))
        .unwrap_or_else(|error| panic!("service: {error}"));
    drop(publication);
    std::fs::remove_file(root.join("native.sock"))
        .unwrap_or_else(|error| panic!("symlink cleanup: {error}"));
    std::fs::remove_file(outside_socket).unwrap_or_else(|error| panic!("native cleanup: {error}"));
    std::fs::remove_dir(root).unwrap_or_else(|error| panic!("root cleanup: {error}"));
    std::fs::remove_dir(outside).unwrap_or_else(|error| panic!("outside cleanup: {error}"));
}
