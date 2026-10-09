//! Real owner-local collaboration API proof for source-affine native route resolution.
use collaboration_client::{protocol::EndpointRef, resolve_public_native_for_endpoint};
use collaboration_service::{CollaborationApplication, ServiceIdentity};
use serde_json::json;
use std::{
    io::ErrorKind,
    os::unix::{fs::PermissionsExt, net::UnixListener},
};

const FIRST_SERVICE: &str = "00000000-0000-4000-8000-000000000001";
const SECOND_SERVICE: &str = "00000000-0000-4000-8000-000000000002";
const EPOCH: &str = "00000000-0000-4000-8000-000000000003";

fn source_endpoint(service_id: &str, endpoint_id: &str) -> Result<EndpointRef, serde_json::Error> {
    serde_json::from_value(json!({"serviceId":service_id,"endpointId":endpoint_id}))
}

#[tokio::test]
async fn native_resolution_accepts_only_the_selected_service_and_endpoint() {
    // Arrange: an actual collaboration API advertises a native socket, without a native session.
    let root = tempfile::Builder::new()
        .prefix("sel-route-")
        .tempdir_in("/tmp")
        .unwrap();
    let directory = std::fs::canonicalize(root.path()).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    let native_socket = UnixListener::bind(directory.join("native.sock")).unwrap();
    native_socket.set_nonblocking(true).unwrap();
    let descriptor = serde_json::from_value(json!({
        "endpoint":source_endpoint(FIRST_SERVICE, "codex-local").unwrap(),
        "label":"Selector source fixture",
        "availability":{"state":"available","observedAt":"2026-10-05T00:00:00Z"},
        "channels":[{"kind":"nativeCodex","transport":"unixWebSocket","path":"native.sock",
            "schemaDigest":null,"generation":{"serviceEpoch":EPOCH,"generation":1}}]
    }))
    .unwrap();
    let identity = ServiceIdentity::new(FIRST_SERVICE, EPOCH)
        .unwrap()
        .with_endpoints(vec![descriptor])
        .unwrap();
    let served = collaboration_mcp::test_support::ServedCollaborationApi::start(
        &directory,
        CollaborationApplication::new(identity),
    )
    .await
    .unwrap();

    // Act/Assert: the opaque session ID is irrelevant; route identity must match both parts.
    for (selected, accepted) in [
        (source_endpoint(FIRST_SERVICE, "codex-local").unwrap(), true),
        (
            source_endpoint(SECOND_SERVICE, "codex-local").unwrap(),
            false,
        ),
        (
            source_endpoint(FIRST_SERVICE, "codex-other").unwrap(),
            false,
        ),
    ] {
        let selected_directory = directory.clone();
        let result = tokio::task::spawn_blocking(move || {
            resolve_public_native_for_endpoint(&selected_directory, &selected)
        })
        .await
        .unwrap();
        if accepted {
            assert_eq!(result.unwrap(), directory.join("native.sock"));
        } else {
            assert!(
                result.is_err(),
                "a foreign service/endpoint cannot select this socket"
            );
        }
    }
    assert_eq!(
        native_socket.accept().unwrap_err().kind(),
        ErrorKind::WouldBlock,
        "route discovery never attaches or creates a session"
    );

    served.stop().await.unwrap();
}
