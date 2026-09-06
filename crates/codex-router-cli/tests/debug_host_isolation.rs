use std::process::Command;

#[test]
#[cfg(debug_assertions)]
fn debug_host_rejects_unsafe_endpoint_before_creating_runtime_state() {
    // Arrange: endpoint-validation fixture; no actual Codex executable or home is used.
    let root = std::env::temp_dir().join(format!(
        "host-isolation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let native_home = root.join("native-home");
    let normal_socket = native_home.join("app-server-control/app-server-control.sock");
    for socket in [None, normal_socket.to_str()] {
        let router_root = root.join("router-state");
        let mut command = Command::new(env!("CARGO_BIN_EXE_codex-router"));
        command
            .arg("host")
            .arg("--router-root")
            .arg(&router_root)
            .env("CODEX_HOME", &native_home)
            .env("PATH", "")
            .env("OTEL_SDK_DISABLED", "true")
            .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
            .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET");
        if let Some(socket) = socket {
            command.env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", socket);
        }
        // Act.
        let output = command.output().unwrap();
        // Assert: destination failure precedes native launch, state/lock creation and launchctl.
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("debug"), "{error}");
        assert!(!router_root.exists());
    }
    assert!(!root.exists());
}

#[test]
fn required_debug_isolation_rejects_home_default_mode() {
    // Arrange / Act: this test must stop in argument/mode validation, before state access.
    let output = Command::new(env!("CARGO_BIN_EXE_codex-router"))
        .args(["host", "--require-debug-isolation"])
        .env("CODEX_ROUTER_USE_HOME_DEFAULT", "1")
        .env("OTEL_SDK_DISABLED", "true")
        .output()
        .unwrap();
    // Assert: the harness cannot silently use installed/home-default launch behavior.
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("debug isolation requires"));
}
