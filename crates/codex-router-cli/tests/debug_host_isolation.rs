use std::process::Command;

#[test]
#[cfg(debug_assertions)]
fn debug_host_rejects_unsafe_endpoint_before_creating_runtime_state() {
    // Arrange: endpoint-validation fixture; no actual Codex executable or home is used.
    let root = std::path::PathBuf::from("/tmp").join(format!(
        "h-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let native_home = root.join("native-home");
    std::fs::create_dir_all(&native_home).unwrap();
    std::fs::write(native_home.join("codex-router-debug.config.toml"), "model_provider = \"codex-router-debug\"\n\n[model_providers.codex-router-debug]\nname = \"OpenAI\"\nbase_url = \"http://127.0.0.1:18787/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nsupports_websockets = true\n\n[features]\nenable_request_compression = false\n").unwrap();
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
        if socket.is_some() {
            assert!(
                error.contains(
                    "debug runtime directory must be dedicated and separate from normal state"
                ),
                "{error}"
            );
        } else {
            assert!(
                error.contains("debug hosted launch requires CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET"),
                "{error}"
            );
        }
        assert!(!router_root.exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn required_debug_isolation_in_home_default_mode_requires_profile() {
    // Arrange / Act: the explicit flag stays isolated even in home-default mode.
    let output = Command::new(env!("CARGO_BIN_EXE_codex-router"))
        .args(["host", "--require-debug-isolation"])
        .env("CODEX_ROUTER_USE_HOME_DEFAULT", "1")
        .env("HOME", std::env::temp_dir().join("missing-host-home"))
        .env(
            "CODEX_HOME",
            std::env::temp_dir().join("missing-host-codex-home"),
        )
        .env("OTEL_SDK_DISABLED", "true")
        .output()
        .unwrap();
    // Assert: the harness cannot silently use installed/home-default launch behavior.
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("isolated Host requires a readable debug profile")
    );
}
