use codex_router_host::{CollaborationRuntime, CollaborationRuntimeInputs};
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn collaboration_runtime_binds_a_fixed_mcp_port_once() {
    let root = std::env::temp_dir().join(format!("mcp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap_or_else(|error| panic!("directory: {error}"));
    let reservation = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|error| panic!("reserve mcp port: {error}"));
    let mcp_bind = reservation
        .local_addr()
        .unwrap_or_else(|error| panic!("mcp address: {error}"));
    drop(reservation);

    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("backend.sock"),
        mcp_bind,
        native_schema: None,
    })
    .await
    .unwrap_or_else(|error| panic!("communication startup: {error}"));

    runtime
        .shutdown()
        .await
        .unwrap_or_else(|error| panic!("shutdown: {error}"));
    let _ = std::fs::remove_dir_all(&root);
}
