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

#[test]
fn debug_endpoint_accepts_distinct_codex_rendezvous_symlinks_but_rejects_normal_target() {
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "endpoint-rendezvous-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let home = root.join("native-home");
    let normal_directory = home.join("app-server-control");
    let debug_directory = root.join("debug-owner");
    let physical_directory = root.join("private-codex-sockets");
    for directory in [&normal_directory, &debug_directory, &physical_directory] {
        std::fs::create_dir_all(directory).unwrap();
    }
    let normal_target = physical_directory.join("normal.sock");
    let debug_target = physical_directory.join("debug.sock");
    std::fs::write(&normal_target, []).unwrap();
    std::fs::write(&debug_target, []).unwrap();
    let paths = CodexPaths::from_codex_home(home.clone());
    let normal_socket = paths.app_server_socket();
    let debug_socket = debug_directory.join("app.sock");
    std::os::unix::fs::symlink(&normal_target, &normal_socket).unwrap();
    std::os::unix::fs::symlink(&debug_target, &debug_socket).unwrap();

    assert_eq!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: debug_socket.to_str(),
        })
        .unwrap(),
        debug_socket
    );

    std::fs::remove_file(&debug_socket).unwrap();
    std::os::unix::fs::symlink(&normal_target, &debug_socket).unwrap();
    assert!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: debug_socket.to_str(),
        })
        .is_err()
    );

    let other_normal_socket = normal_directory.join("other.sock");
    std::fs::write(&other_normal_socket, []).unwrap();
    std::fs::remove_file(&debug_socket).unwrap();
    std::os::unix::fs::symlink(&other_normal_socket, &debug_socket).unwrap();
    assert!(
        select_app_server_endpoint(AppServerEndpointSelection {
            paths: &paths,
            debug_defaults: true,
            requested_socket: debug_socket.to_str(),
        })
        .is_err()
    );

    for path in [
        debug_socket,
        normal_socket,
        other_normal_socket,
        debug_target,
        normal_target,
    ] {
        std::fs::remove_file(path).unwrap();
    }
    for directory in [
        physical_directory,
        debug_directory,
        normal_directory,
        home,
        root,
    ] {
        std::fs::remove_dir(directory).unwrap();
    }
}
