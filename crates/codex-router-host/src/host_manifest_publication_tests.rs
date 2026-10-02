use crate::{CollaborationRuntime, CollaborationRuntimeInputs, HostCollaborationInputs};
use collaboration_protocol::{RouterExecutableRelation, ServiceManifest};
use std::{net::SocketAddr, os::unix::fs::PermissionsExt as _};

fn runtime_inputs(directory: &std::path::Path) -> CollaborationRuntimeInputs {
    CollaborationRuntimeInputs {
        directory: directory.to_owned(),
        codex_home: directory.to_owned(),
        backend_socket: directory.join("backend.sock"),
        mcp_bind: SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: Some(directory.join("claude-sessions")),
        remote_control_server_name: None,
        owner_human_id: None,
    }
}

fn service_manifest(directory: &std::path::Path) -> ServiceManifest {
    serde_json::from_slice(
        &std::fs::read(directory.join("service.json")).expect("published service manifest"),
    )
    .expect("typed service manifest")
}

#[tokio::test]
async fn host_startup_publishes_the_configured_router_proxy_port() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");
    let router_proxy_endpoint: SocketAddr = "127.0.0.1:18787".parse().expect("custom Router port");
    let (_relation_sender, relation_receiver) =
        tokio::sync::watch::channel(RouterExecutableRelation::Match);

    let runtime = CollaborationRuntime::start_for_host_with_router_proxy_endpoint(
        HostCollaborationInputs {
            collaboration_runtime: runtime_inputs(root.path()),
            router_proxy_endpoint,
        },
        Vec::new(),
        relation_receiver,
    )
    .await
    .expect("Host collaboration runtime");
    let manifest = service_manifest(root.path());
    runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(manifest.router_proxy_endpoint, Some(router_proxy_endpoint));
}

#[tokio::test]
async fn direct_runtime_publication_omits_the_host_router_proxy_endpoint() {
    let root = tempfile::tempdir().expect("runtime root");
    std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
        .expect("private runtime root");

    let runtime = CollaborationRuntime::start(runtime_inputs(root.path()))
        .await
        .expect("direct collaboration runtime");
    let manifest = service_manifest(root.path());
    runtime.shutdown().await.expect("runtime shutdown");

    assert_eq!(manifest.router_proxy_endpoint, None);
}
