use codex_native_integration::{
    AppServerEndpointSelection, CodexPaths, select_app_server_endpoint,
};

#[test]
fn debug_endpoint_rejects_normal_directory_alias_and_preserves_installed_selection() {
    // Arrange: controlled path fixtures only, no real Codex home or sockets.
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "endpoint-selection-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let home = root.join("native-home");
    let normal_directory = home.join("app-server-control");
    std::fs::create_dir_all(&normal_directory).unwrap();
    let alias = root.join("normal-alias");
    std::os::unix::fs::symlink(&normal_directory, &alias).unwrap();
    let paths = CodexPaths::from_codex_home(home.clone());
    let alias_socket = alias.join("another.sock");
    let safe_socket = root.join("debug-owner/backend.sock");
    // Act / Assert: a different filename through an alias is still the normal directory.
    assert!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: alias_socket.to_str()
        })
        .is_err()
    );
    assert!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: None
        })
        .is_err()
    );
    assert_eq!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: safe_socket.to_str()
        })
        .unwrap(),
        safe_socket
    );
    assert_eq!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: false,
            requested_socket: safe_socket.to_str()
        })
        .unwrap(),
        paths.app_server_socket()
    );
    std::fs::remove_file(alias).unwrap();
    std::fs::remove_dir(normal_directory).unwrap();
    std::fs::remove_dir(home).unwrap();
    std::fs::remove_dir(root).unwrap();
}
