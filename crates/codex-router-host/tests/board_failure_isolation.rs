use codex_router_host::{CollaborationRuntime, CollaborationRuntimeInputs};
use collaboration_client::{BoardClientError, ControlClient};
use message_board::{BoardFailureKind, PageLimit, PageRequest, ProjectListRequest};
use std::os::unix::fs::DirBuilderExt;

#[tokio::test]
async fn board_open_failure_keeps_unrelated_control_methods_available()
-> Result<(), Box<dyn std::error::Error>> {
    let root = std::path::PathBuf::from(format!(
        "/tmp/host-board-failure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    std::fs::create_dir(root.join("project-board.sqlite"))?;
    let runtime = CollaborationRuntime::start(CollaborationRuntimeInputs {
        directory: root.clone(),
        codex_home: root.clone(),
        backend_socket: root.join("absent-native.sock"),
        mcp_bind: std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        native_schema: None,
        peer_registry_directory: None,
    })
    .await?;
    let mut client = ControlClient::connect(&root, "board-failure-isolation", "1").await?;

    let _endpoints = client.list_endpoints().await?;
    let board_result = client
        .board_project_list(ProjectListRequest {
            repository: None,
            page: PageRequest {
                limit: PageLimit::try_from(10)?,
                cursor: None,
            },
        })
        .await;
    let _close = client.close().await;
    runtime.shutdown().await?;
    if root.join("provider-operations.sqlite-wal").exists()
        || root.join("provider-operations.sqlite-shm").exists()
    {
        return Err("provider operation sidecars outlived runtime shutdown".into());
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir(entry.path())?;
        } else {
            std::fs::remove_file(entry.path())?;
        }
    }
    std::fs::remove_dir(root)?;
    if !matches!(
        board_result,
        Err(BoardClientError::Rejected(error)) if error.kind == BoardFailureKind::BoardUnavailable
    ) {
        return Err(
            "unavailable board store did not return the typed boardUnavailable error".into(),
        );
    }
    Ok(())
}
