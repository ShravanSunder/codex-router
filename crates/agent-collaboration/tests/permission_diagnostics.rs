use std::os::unix::fs::PermissionsExt;

struct DeniedServiceDirectory {
    root: std::path::PathBuf,
    manifest: std::path::PathBuf,
}

impl DeniedServiceDirectory {
    fn create(label: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "collaboration-permission-{label}-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir(&root)?;
        let manifest = root.join("service.json");
        std::fs::write(&manifest, b"{}")?;
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o000))?;
        Ok(Self { root, manifest })
    }
}

impl Drop for DeniedServiceDirectory {
    fn drop(&mut self) {
        let _restored =
            std::fs::set_permissions(&self.manifest, std::fs::Permissions::from_mode(0o600));
        let _manifest_removed = std::fs::remove_file(&self.manifest);
        let _root_removed = std::fs::remove_dir(&self.root);
    }
}

#[test]
fn endpoint_discovery_reports_typed_permission_recovery_in_json_and_human_output() {
    let fixture = DeniedServiceDirectory::create("endpoints")
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let binary = env!("CARGO_BIN_EXE_agent-collaboration");

    let machine = std::process::Command::new(binary)
        .args(["endpoints", "list", "--json", "--service-directory"])
        .arg(&fixture.root)
        .output()
        .unwrap_or_else(|error| panic!("machine command: {error}"));
    assert_eq!(machine.status.code(), Some(3));
    let record: serde_json::Value = serde_json::from_slice(&machine.stdout)
        .unwrap_or_else(|error| panic!("machine output: {error}"));
    assert_eq!(record["error"]["kind"], "permissionDenied");
    assert_eq!(record["error"]["stage"], "manifestRead");
    assert_eq!(record["error"]["nextAction"], "requestApproval");
    assert!(
        record["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("automated approval review"))
    );

    let human = std::process::Command::new(binary)
        .args(["endpoints", "list", "--service-directory"])
        .arg(&fixture.root)
        .output()
        .unwrap_or_else(|error| panic!("human command: {error}"));
    assert_eq!(human.status.code(), Some(3));
    let stderr =
        String::from_utf8(human.stderr).unwrap_or_else(|error| panic!("human stderr: {error}"));
    assert!(stderr.contains("Error: permissionDenied"));
    assert!(stderr.contains("Stage: manifestRead"));
    assert!(stderr.contains("Next action: requestApproval"));
    assert!(stderr.contains("automated approval review"));
    assert!(stderr.contains("ask the user"));
}

#[test]
fn wake_permission_diagnostic_preserves_operation_id_and_pre_dispatch_exit() {
    let fixture =
        DeniedServiceDirectory::create("wake").unwrap_or_else(|error| panic!("fixture: {error}"));
    let operation_id = "019f0000-0000-7000-8000-000000000001";
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "wake",
            "pause",
            "--wakeup-id",
            "019f0000-0000-7000-8000-000000000002",
            "--operation-id",
            operation_id,
            "--json",
            "--service-directory",
        ])
        .arg(&fixture.root)
        .output()
        .unwrap_or_else(|error| panic!("wake command: {error}"));

    assert_eq!(output.status.code(), Some(3));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("wake output: {error}"));
    assert_eq!(record["operationId"], operation_id);
    assert_eq!(record["error"]["kind"], "permissionDenied");
    assert_eq!(record["error"]["nextAction"], "requestApproval");
}

#[test]
fn wake_read_permission_diagnostic_preserves_explicit_null_operation_id() {
    let fixture = DeniedServiceDirectory::create("wake-show")
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args([
            "wake",
            "show",
            "--wakeup-id",
            "019f0000-0000-7000-8000-000000000002",
            "--json",
            "--service-directory",
        ])
        .arg(&fixture.root)
        .output()
        .unwrap_or_else(|error| panic!("wake command: {error}"));

    assert_eq!(output.status.code(), Some(3));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("wake output: {error}"));
    assert_eq!(record.get("operationId"), Some(&serde_json::Value::Null));
    assert_eq!(record["error"]["kind"], "permissionDenied");
}

#[test]
fn stream_and_raw_carrier_entrypoints_report_initial_permission_denial() {
    let fixture = DeniedServiceDirectory::create("stream-carriers")
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let directory = fixture.root.to_string_lossy().into_owned();
    let binary = env!("CARGO_BIN_EXE_agent-collaboration");

    for command in [
        vec![
            "events",
            "listen",
            "--endpoint",
            "codex-local",
            "--session",
            "permission-fixture",
            "--attach",
            "--service-directory",
            &directory,
        ],
        vec![
            "conversation",
            "prompt",
            "--endpoint",
            "codex-local",
            "--new",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "medium",
            "--access",
            "workspace-write",
            "--cwd",
            "/tmp",
            "--text",
            "fixture",
            "--json",
            "--service-directory",
            &directory,
        ],
    ] {
        let output = std::process::Command::new(binary)
            .args(&command)
            .output()
            .unwrap_or_else(|error| panic!("stream command {command:?}: {error}"));
        assert_eq!(output.status.code(), Some(3), "command {command:?}");
        let records = serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_else(|error| panic!("stream output {command:?}: {error}"));
        if command.first() == Some(&"conversation") {
            assert_eq!(records[0]["kind"], "conversationOperationStarted");
            assert!(records[0]["operationId"].as_str().is_some());
        }
        let record = records.last().expect("permission failure record");
        assert_eq!(record["error"]["kind"], "permissionDenied");
        assert_eq!(record["error"]["nextAction"], "requestApproval");
    }

    for command in [
        vec![
            "acp",
            "--endpoint",
            "codex-local",
            "--service-directory",
            &directory,
        ],
        vec![
            "native",
            "--endpoint",
            "codex-local",
            "--service-directory",
            &directory,
        ],
    ] {
        let output = std::process::Command::new(binary)
            .args(&command)
            .output()
            .unwrap_or_else(|error| panic!("raw command {command:?}: {error}"));
        assert_eq!(output.status.code(), Some(3), "command {command:?}");
        assert!(output.stdout.is_empty(), "raw stdout {command:?}");
        let stderr = String::from_utf8(output.stderr)
            .unwrap_or_else(|error| panic!("raw stderr {command:?}: {error}"));
        assert!(stderr.contains("permissionDenied"), "command {command:?}");
        assert!(stderr.contains("requestApproval"), "command {command:?}");
    }
}

#[test]
#[cfg(debug_assertions)]
fn hosted_native_launcher_reports_initial_permission_denial() {
    let router_root = std::env::temp_dir().join(format!(
        "collaboration-launch-permission-{}",
        uuid::Uuid::now_v7()
    ));
    std::fs::create_dir(&router_root).unwrap_or_else(|error| panic!("router root: {error}"));
    let app_server_owner = std::path::PathBuf::from(format!("/tmp/cpd-app-{}", std::process::id()));
    std::fs::create_dir(&app_server_owner)
        .unwrap_or_else(|error| panic!("app-server owner: {error}"));
    {
        let service_root = router_root.join("agent-communication");
        std::fs::create_dir(&service_root).unwrap_or_else(|error| panic!("service root: {error}"));
        let manifest = service_root.join("service.json");
        std::fs::write(&manifest, b"{}")
            .unwrap_or_else(|error| panic!("service manifest: {error}"));
        std::fs::set_permissions(&manifest, std::fs::Permissions::from_mode(0o000))
            .unwrap_or_else(|error| panic!("deny manifest: {error}"));
        let _fixture = DeniedServiceDirectory {
            root: service_root,
            manifest,
        };

        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .arg("--new")
            .env("CODEX_HOME", router_root.join("codex-home"))
            .env("CODEX_ROUTER_DEBUG_ROUTER_ROOT", &router_root)
            .env(
                "CODEX_ROUTER_DEBUG_APP_SERVER_SOCKET",
                app_server_owner.join("app-server.sock"),
            )
            .env_remove("CODEX_ROUTER_USE_HOME_DEFAULT")
            .env("PATH", "")
            .output()
            .unwrap_or_else(|error| panic!("launcher command: {error}"));

        assert_eq!(
            output.status.code(),
            Some(3),
            "stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr)
            .unwrap_or_else(|error| panic!("launcher stderr: {error}"));
        assert!(stderr.contains("permissionDenied"));
        assert!(stderr.contains("requestApproval"));
    }
    std::fs::remove_dir(&app_server_owner)
        .unwrap_or_else(|error| panic!("app-server cleanup: {error}"));
    std::fs::remove_dir(&router_root).unwrap_or_else(|error| panic!("router cleanup: {error}"));
}

#[test]
fn missing_service_remains_unavailable() {
    let root = std::env::temp_dir().join(format!("collaboration-absent-{}", uuid::Uuid::now_v7()));
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
        .args(["endpoints", "list", "--json", "--service-directory"])
        .arg(root)
        .output()
        .unwrap_or_else(|error| panic!("endpoint command: {error}"));

    assert_eq!(output.status.code(), Some(3));
    let record: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("endpoint output: {error}"));
    assert_eq!(record["error"]["kind"], "unavailable");
    assert_eq!(record["error"]["message"], "Endpoint discovery unavailable");
}

#[test]
fn control_command_families_share_permission_diagnostic() {
    let fixture = DeniedServiceDirectory::create("control-families")
        .unwrap_or_else(|error| panic!("fixture: {error}"));
    let directory = fixture.root.to_string_lossy().into_owned();
    let uuid = "019f0000-0000-7000-8000-000000000003";
    let address = serde_json::json!({
        "endpoint": {
            "serviceId": "019f0000-0000-7000-8000-000000000004",
            "endpointId": "codex-local"
        },
        "sessionId": "permission-fixture"
    })
    .to_string();
    let commands: Vec<Vec<&str>> = vec![
        vec![
            "addresses",
            "list",
            "--endpoint",
            "codex-local",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "journal",
            "status",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "sessions",
            "list",
            "--endpoint",
            "codex-local",
            "--view",
            "stored",
            "--any",
            "--source",
            "all",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "session",
            "inspect",
            "--endpoint",
            "codex-local",
            "--session",
            "permission-fixture",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "instruction",
            "show",
            "--instruction-id",
            uuid,
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "run",
            "show",
            "--run-id",
            uuid,
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "schedule",
            "show",
            "--schedule-id",
            uuid,
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "automation",
            "status",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "board",
            "project",
            "list",
            "--json",
            "--service-directory",
            &directory,
        ],
        vec![
            "message",
            "send",
            "--to",
            &address,
            "--from",
            &address,
            "--text",
            "permission fixture",
            "--json",
            "--service-directory",
            &directory,
        ],
    ];

    for command in commands {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_agent-collaboration"))
            .args(&command)
            .output()
            .unwrap_or_else(|error| panic!("command {command:?}: {error}"));
        assert_eq!(output.status.code(), Some(3), "command {command:?}");
        let record: serde_json::Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("output for {command:?}: {error}"));
        assert_eq!(
            record["error"]["kind"], "permissionDenied",
            "command {command:?}: {record}"
        );
        assert_eq!(record["error"]["nextAction"], "requestApproval");
    }
}
