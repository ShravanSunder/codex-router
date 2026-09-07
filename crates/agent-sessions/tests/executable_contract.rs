use std::process::Command;

#[test]
#[cfg(debug_assertions)]
fn debug_hosted_dry_run_refuses_missing_or_production_endpoint() {
    // Arrange: dry-run must fail before ever launching Codex or reading history.
    for socket in [
        None,
        Some("/tmp/debug-selection-home/app-server-control/app-server-control.sock"),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-sessions"));
        command
            .args(["--new", "--dry-run"])
            .env("CODEX_HOME", "/tmp/debug-selection-home")
            .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
            .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET");
        if let Some(socket) = socket {
            command.env("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET", socket);
        }
        // Act.
        let output = command.output().unwrap();
        // Assert: a debug profile never silently selects the normal backend.
        assert!(
            !output.status.success(),
            "unsafe endpoint was accepted by debug dry-run"
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("debug"));
    }
}

#[test]
#[cfg(debug_assertions)]
fn local_debug_dry_run_keeps_profile_without_a_hosted_endpoint() {
    // Arrange / Act: local embedded Codex needs no external backend endpoint.
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args(["--local", "--new", "--dry-run"])
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .env("PATH", "")
        .output()
        .unwrap();
    // Assert: retain profile semantics; do not synthesize remote attachment.
    assert!(output.status.success());
    let launch = String::from_utf8_lossy(&output.stdout);
    assert!(launch.contains("--profile codex-router-debug"));
    assert!(!launch.contains("--remote"));
}

#[test]
fn version_is_a_successful_standalone_operation() {
    // Arrange / Act
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("version command should execute: {error}"));
    // Assert
    assert!(output.status.success());
    assert_eq!(
        output.stdout,
        format!("agent-sessions {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn help_uses_the_independent_executable_name() {
    // Arrange / Act
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .arg("--help")
        .output()
        .unwrap_or_else(|error| panic!("help command should execute: {error}"));
    // Assert
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("agent-sessions"));
    assert!(help.contains("--dry-run"));
    assert!(help.contains("--local"));
    assert!(output.stderr.is_empty());
}

#[test]
#[cfg(debug_assertions)]
fn debug_dry_run_preserves_isolated_socket_and_never_executes_codex() {
    // Arrange: deliberately no executable search path; dry-run must not launch Codex.
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
        .env_remove("CODEX_HOME")
        .env(
            "CODEX_ROUTER_DEBUG_ROUTER_ROOT",
            "/tmp/session-proof-router",
        )
        .env("PATH", "")
        .env(
            "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET",
            "/tmp/session-proof-owner/backend.sock",
        )
        .args(["--new", "--dry-run"])
        // Act
        .output()
        .unwrap_or_else(|error| panic!("dry-run should execute: {error}"));
    // Assert
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let launch = String::from_utf8_lossy(&output.stdout);
    assert!(launch.contains("--profile codex-router-debug"));
    assert!(launch.contains(
        "--remote unix:///tmp/session-proof-router/agent-communication/codex-native.sock"
    ));
    assert!(output.stderr.is_empty());
}

#[test]
fn message_discovery_error_reports_category_without_leaking_content_or_path() {
    // Arrange: nonexistent service directory; no native process or send can occur.
    let id = "00000000-0000-4000-8000-000000000001";
    let target = serde_json::json!({"endpoint":{"serviceId":id,"endpointId":"codex-local"},"sessionId":"test"}).to_string();
    let absent = format!("/tmp/absent-message-service-{}", std::process::id());
    // Act.
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .args([
            "message",
            "send",
            "--human-user",
            "--to",
            &target,
            "--text",
            "private message text",
            "--service-directory",
            &absent,
            "--json",
        ])
        .output()
        .unwrap();
    // Assert: useful non-secret failure category, no arbitrary IO message text.
    assert_eq!(output.status.code(), Some(3));
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("NotFound"));
    assert!(text.contains("manifest-read"));
    assert!(!text.contains(&absent));
    assert!(!text.contains("private message text"));
}

#[test]
fn event_listening_requires_explicit_attachment_and_reports_missing_service() {
    // Arrange: no service exists; listing events must never imply a passive native read.
    let binary = env!("CARGO_BIN_EXE_agent-sessions");
    // Act: omitted attachment is rejected by argument parsing.
    let missing_consent = Command::new(binary)
        .args([
            "events",
            "listen",
            "--endpoint",
            "codex-local",
            "--session",
            "thread",
        ])
        .output()
        .unwrap();
    // Assert: no connection can be attempted without the explicit effect flag.
    assert_eq!(missing_consent.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&missing_consent.stderr).contains("--attach"));
    let missing_service = Command::new(binary)
        .args([
            "events",
            "listen",
            "--endpoint",
            "codex-local",
            "--session",
            "thread",
            "--attach",
            "--service-directory",
            "/tmp/missing-observation-fixture/no-service",
        ])
        .output()
        .unwrap();
    assert_eq!(missing_service.status.code(), Some(3));
    let output = String::from_utf8(missing_service.stdout).unwrap();
    assert!(!output.contains("listenerReady"));
    assert!(output.contains("unavailable"));
}

#[test]
fn stored_listing_reaches_catalog_without_host_launch_configuration() {
    // Arrange: only a Codex home is supplied; its absent database is the expected failure.
    let output = Command::new(env!("CARGO_BIN_EXE_agent-sessions"))
        .env_remove("HOME")
        .env_remove("CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET")
        .env_remove("CODEX_ROUTER_DEBUG_ROUTER_ROOT")
        .env("CODEX_HOME", "/tmp/absent-codex-catalog-fixture")
        .args(["--list", "--any", "--format", "json"])
        .output()
        .unwrap();
    // Assert: listing reaches read-only catalog IO, not unrelated Host/profile preflight.
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(error.contains("database"), "{error}");
    assert!(!error.contains("debug app-server"));
}
