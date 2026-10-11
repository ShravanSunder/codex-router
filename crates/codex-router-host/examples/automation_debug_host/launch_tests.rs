use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn explicit_root_and_acp_command_are_admitted_for_isolated_interaction_proof() {
    let result = parse_options_from(
        [
            "--run-directory",
            "/tmp/sqlite-proof",
            "--router-binary",
            "/tmp/router",
            "--router-root",
            "/tmp/sqlite-proof-account",
            "--claude-acp-executable",
            "/opt/homebrew/bin/pnpm",
            "--claude-acp-arguments",
            "dlx",
            "--claude-acp-arguments",
            "@agentclientprotocol/claude-agent-acp@0.81.2",
        ]
        .map(OsString::from),
    );
    assert!(
        result.is_ok(),
        "explicit isolated ACP launch was rejected: {result:?}"
    );
}

fn debug_profile(endpoint: &str) -> String {
    format!(
        r#"
model = "existing-model"
model_reasoning_effort = "low"
model_provider = "codex-router-debug"
[model_providers.codex-router-debug]
name = "OpenAI"
base_url = "{endpoint}"
wire_api = "responses"
requires_openai_auth = false
supports_websockets = true

[features]
enable_request_compression = false
"#
    )
}

#[test]
fn luna_profile_validates_saved_route_before_in_memory_retargeting() {
    assert!(luna_profile_from_text(&debug_profile("http://127.0.0.1:18787/v1"), 28787).is_ok());
    for (saved_endpoint, selected_port) in [
        ("http://127.0.0.1:8787/v1", 28787),
        ("http://0.0.0.0:18787/v1", 28787),
        ("not-a-url", 28787),
        ("http://127.0.0.1:18787/v1", 8787),
    ] {
        assert!(
            luna_profile_from_text(&debug_profile(saved_endpoint), selected_port).is_err(),
            "invalid saved/selected route was admitted: {saved_endpoint} -> {selected_port}"
        );
    }
}

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestRunDirectory(PathBuf);

impl TestRunDirectory {
    fn create() -> Result<Self, Box<dyn std::error::Error>> {
        let root = PathBuf::from("/tmp").join(format!(
            "automation-debug-host-resume-{}-{}",
            std::process::id(),
            NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&root)?;
        for child in ["agent-communication", "agent-workspace", "native-socket"] {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(root.join(child))?;
        }
        Ok(Self(root))
    }

    fn write_context(&self, process_id: u32) -> Result<(), Box<dyn std::error::Error>> {
        write_fresh_context(&DebugHostContext {
            kind: "debugHostPrepared".to_owned(),
            run_directory: self.0.clone(),
            service_directory: self.0.join("agent-communication"),
            workspace: self.0.join("agent-workspace"),
            profile: "codex-router-debug".to_owned(),
            model: "gpt-5.6-luna".to_owned(),
            port: 18787,
            host_pid: process_id,
            router_root: None,
            claude_provider: None,
        })
    }
}

impl Drop for TestRunDirectory {
    fn drop(&mut self) {
        let _cleanup = std::fs::remove_dir_all(&self.0);
    }
}

fn stopped_process_id() -> Result<u32, Box<dyn std::error::Error>> {
    let mut child = std::process::Command::new("/usr/bin/true").spawn()?;
    let process_id = child.id();
    let _status = child.wait()?;
    Ok(process_id)
}

#[test]
fn run_directory_modes_are_mutually_exclusive() {
    let result = parse_options_from([
        OsString::from("--run-directory"),
        OsString::from("/tmp/fresh"),
        OsString::from("--resume-run-directory"),
        OsString::from("/tmp/resume"),
    ]);
    assert!(
        result
            .expect_err("conflicting run-directory modes must fail")
            .to_string()
            .contains("mutually exclusive")
    );
}

#[test]
fn resume_rejects_marker_for_another_root() -> Result<(), Box<dyn std::error::Error>> {
    let run = TestRunDirectory::create()?;
    let foreign_run = TestRunDirectory::create()?;
    run.write_context(stopped_process_id()?)?;
    let marker_path = run.0.join("debug-host-context.json");
    let mut marker: DebugHostContext = serde_json::from_slice(&std::fs::read(&marker_path)?)?;
    marker.run_directory = foreign_run.0.clone();
    std::fs::write(&marker_path, serde_json::to_vec(&marker)?)?;

    let error = validate_resume_run_directory(&run.0, 18787).expect_err("foreign marker must fail");
    assert!(error.to_string().contains("does not match"));
    Ok(())
}

#[test]
fn resume_rejects_invalid_context_marker() -> Result<(), Box<dyn std::error::Error>> {
    let run = TestRunDirectory::create()?;
    run.write_context(stopped_process_id()?)?;
    let marker_path = run.0.join("debug-host-context.json");
    let mut marker: DebugHostContext = serde_json::from_slice(&std::fs::read(&marker_path)?)?;
    marker.model = "not-luna".to_owned();
    std::fs::write(&marker_path, serde_json::to_vec(&marker)?)?;

    let error = validate_resume_run_directory(&run.0, 18787).expect_err("invalid marker must fail");
    assert!(error.to_string().contains("does not match"));
    Ok(())
}

#[test]
fn resume_rejects_live_prior_host_process() -> Result<(), Box<dyn std::error::Error>> {
    let run = TestRunDirectory::create()?;
    run.write_context(std::process::id())?;

    let error =
        validate_resume_run_directory(&run.0, 18787).expect_err("live marker PID must fail");
    assert!(error.to_string().contains("still alive"));
    Ok(())
}

#[test]
fn resume_admits_stopped_owned_private_root() -> Result<(), Box<dyn std::error::Error>> {
    let run = TestRunDirectory::create()?;
    run.write_context(stopped_process_id()?)?;

    validate_resume_run_directory(&run.0, 18787)?;
    Ok(())
}

#[test]
fn resume_replaces_marker_without_changing_service_data() -> Result<(), Box<dyn std::error::Error>>
{
    let run = TestRunDirectory::create()?;
    run.write_context(stopped_process_id()?)?;
    let database_path = run.0.join("agent-communication/project-board.sqlite");
    std::fs::write(&database_path, b"preserved-board-state")?;
    let resumed_context = DebugHostContext {
        kind: "debugHostPrepared".to_owned(),
        run_directory: run.0.clone(),
        service_directory: run.0.join("agent-communication"),
        workspace: run.0.join("agent-workspace"),
        profile: "codex-router-debug".to_owned(),
        model: "gpt-5.6-luna".to_owned(),
        port: 18787,
        host_pid: 4242,
        router_root: None,
        claude_provider: None,
    };

    replace_resumed_context(&resumed_context)?;

    let published = read_existing_context(&run.0)?;
    assert_eq!(published.host_pid, 4242);
    assert_eq!(std::fs::read(database_path)?, b"preserved-board-state");
    Ok(())
}

#[test]
fn resume_rejects_symlinked_context_marker() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let run = TestRunDirectory::create()?;
    let target = run.0.join("context-target.json");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&target)?;
    symlink(&target, run.0.join("debug-host-context.json"))?;

    let error = validate_resume_run_directory(&run.0, 18787).expect_err("symlink marker must fail");
    assert!(error.to_string().contains("regular file"));
    Ok(())
}

fn isolated_acp_options() -> launch_options::DebugHostOptions {
    parse_options_from(
        [
            "--run-directory",
            "/tmp/sqlite-proof",
            "--router-binary",
            "/tmp/router",
            "--router-root",
            "/tmp/sqlite-proof-account",
            "--port",
            "28787",
            "--claude-acp-executable",
            "/opt/homebrew/bin/pnpm",
            "--claude-acp-arguments",
            "dlx",
            "--claude-acp-arguments",
            "@agentclientprotocol/claude-agent-acp@0.81.2",
        ]
        .map(OsString::from),
    )
    .expect("valid ACP options")
    .expect("launch options")
}

#[test]
fn isolated_acp_options_attach_actual_provider_and_global_selector() {
    let options = isolated_acp_options();
    let config = HostConfig::new(HostConfigInputs {
        coordination_paths: options.host_coordination_paths(Path::new("/tmp/sqlite-proof-account")),
        router_endpoint: "127.0.0.1:28787".parse().expect("loopback endpoint"),
        mcp_bind: "127.0.0.1:0".parse().expect("ephemeral MCP"),
        app_server_socket: "/tmp/native.sock".into(),
        managed_executable: "/tmp/codex".into(),
        deadlines: HostDeadlines::production(),
    });
    let configured = options
        .attach_provider(config.clone(), Some("/global/claude".into()))
        .expect("provider attachment");
    assert_eq!(
        configured.coordination_paths().router_secret_root(),
        Some(PathBuf::from("/tmp/sqlite-proof-account/secrets"))
    );
    assert_eq!(configured.external_provider_startups().len(), 1);
    let codex_router_host::ExternalProviderStartup::Launch(binding) =
        &configured.external_provider_startups()[0]
    else {
        panic!("real launch binding required")
    };
    assert_eq!(
        binding.launch.executable,
        PathBuf::from("/opt/homebrew/bin/pnpm")
    );
    assert_eq!(
        binding.launch.arguments,
        ["dlx", "@agentclientprotocol/claude-agent-acp@0.81.2"]
    );
    assert_eq!(
        binding.launch.environment,
        [(
            "CLAUDE_CODE_EXECUTABLE".to_owned(),
            "/global/claude".to_owned()
        )]
    );
    assert!(options.attach_provider(config.clone(), None).is_err());
    assert!(
        options
            .attach_provider(config, Some("relative/claude".into()))
            .is_err()
    );
}

#[test]
fn isolated_router_child_uses_selected_root_and_debug_isolation() {
    let options = isolated_acp_options();
    let expected = ChildCommandSpec::new("/tmp/router".into())
        .with_arguments([
            "serve",
            "--require-debug-isolation",
            "--port",
            "28787",
            "--state-db",
            "/tmp/sqlite-proof-account/state.sqlite",
            "--secret-root",
            "/tmp/sqlite-proof-account/secrets",
            "--audit-file",
            "/tmp/sqlite-proof/router-audit.jsonl",
        ])
        .with_output(ChildOutput::Telemetry);
    assert_eq!(
        options.router_child_command(Path::new("/tmp/sqlite-proof-account")),
        expected
    );
}

#[test]
fn host_and_router_share_declared_root_without_copying_tokens() {
    let options = isolated_acp_options();
    let paths = options.host_coordination_paths(Path::new("/tmp/sqlite-proof-account"));
    assert_eq!(
        paths.router_secret_root(),
        Some(PathBuf::from("/tmp/sqlite-proof-account/secrets"))
    );
    assert_eq!(
        paths.instance_lock(),
        Path::new("/tmp/sqlite-proof-account/host.lock")
    );
    assert_eq!(
        paths.operator_socket(),
        Path::new("/tmp/sqlite-proof/host.sock")
    );
    let expected_router = ChildCommandSpec::new("/tmp/router".into())
        .with_arguments([
            "serve",
            "--require-debug-isolation",
            "--port",
            "28787",
            "--state-db",
            "/tmp/sqlite-proof-account/state.sqlite",
            "--secret-root",
            "/tmp/sqlite-proof-account/secrets",
            "--audit-file",
            "/tmp/sqlite-proof/router-audit.jsonl",
        ])
        .with_output(ChildOutput::Telemetry);
    assert_eq!(
        options.router_child_command(Path::new("/tmp/sqlite-proof-account")),
        expected_router
    );
}

#[test]
fn resumed_host_preserves_declared_root_local_token_owner() {
    let options = isolated_acp_options();
    let resumed = parse_options_from(options.resume_arguments())
        .expect("resume options")
        .expect("launch");
    let paths = resumed.host_coordination_paths(Path::new("/tmp/sqlite-proof-account"));
    assert_eq!(
        paths.router_secret_root(),
        Some(PathBuf::from("/tmp/sqlite-proof-account/secrets"))
    );
    assert_eq!(
        paths.instance_lock(),
        Path::new("/tmp/sqlite-proof-account/host.lock")
    );
    assert_eq!(
        paths.operator_socket(),
        Path::new("/tmp/sqlite-proof/host.sock")
    );
}

#[test]
fn default_host_uses_resolved_debug_root_for_local_token_owner() {
    let mut options = isolated_acp_options();
    options.router_root = None;
    options.claude_provider = None;
    let paths = options.host_coordination_paths(Path::new("/tmp/default-debug-account"));
    assert_eq!(
        paths.router_secret_root(),
        Some(PathBuf::from("/tmp/default-debug-account/secrets"))
    );
    assert_eq!(
        paths.instance_lock(),
        Path::new("/tmp/default-debug-account/host.lock")
    );
    assert_eq!(
        paths.operator_socket(),
        Path::new("/tmp/sqlite-proof/host.sock")
    );
}

#[test]
fn replacement_preserves_root_provider_and_port_without_shared_fallback() {
    let options = isolated_acp_options();
    let resumed = parse_options_from(options.resume_arguments())
        .expect("resume options")
        .expect("launch");
    assert_eq!(resumed.router_root, options.router_root);
    assert_eq!(resumed.claude_provider, options.claude_provider);
    assert_eq!(resumed.port, options.port);
    assert_eq!(resumed.run_directory, options.run_directory);
    assert_eq!(
        resumed.run_directory_admission,
        RunDirectoryAdmission::Resume
    );
}

#[test]
fn resume_refuses_changed_credential_root_or_provider_command()
-> Result<(), Box<dyn std::error::Error>> {
    let run = TestRunDirectory::create()?;
    let mut options = isolated_acp_options();
    run.write_context(stopped_process_id()?)?;
    let mut context = read_existing_context(&run.0)?;
    context.router_root = options.router_root.clone();
    context.claude_provider = options.claude_provider.clone();
    validate_resume_binding(&context, &options)?;
    options.router_root = Some("/tmp/another-store".into());
    assert!(validate_resume_binding(&context, &options).is_err());
    options.router_root = context.router_root.clone();
    options
        .claude_provider
        .as_mut()
        .expect("provider")
        .arguments
        .push("--different".into());
    assert!(validate_resume_binding(&context, &options).is_err());
    Ok(())
}

#[test]
fn acp_options_require_explicit_absolute_root_and_executable() {
    for extras in [
        vec!["--claude-acp-executable", "/absolute/acp"],
        vec!["--router-root", "relative/root"],
        vec![
            "--router-root",
            "/tmp/owned",
            "--claude-acp-executable",
            "relative/acp",
        ],
        vec!["--claude-acp-arguments", "dlx"],
    ] {
        let mut arguments = vec![
            "--run-directory",
            "/tmp/proof",
            "--router-binary",
            "/tmp/router",
        ];
        arguments.extend(extras);
        assert!(parse_options_from(arguments.into_iter().map(OsString::from)).is_err());
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ObservedNativeFeatureControls {
    hooks_enabled: Option<bool>,
    request_compression_enabled: Option<bool>,
    network_proxy_enabled: Option<bool>,
}

fn observe_ordered_native_feature_controls(
    arguments: &[OsString],
) -> ObservedNativeFeatureControls {
    let mut observed = ObservedNativeFeatureControls::default();
    for pair in arguments.windows(2).filter(|pair| pair[0] == "-c") {
        let assignment = pair[1].to_str().expect("UTF-8 native override");
        let (key, literal) = assignment.split_once('=').expect("override assignment");
        let value: toml::Table = format!("value={literal}")
            .parse()
            .expect("literal override");
        let value = &value["value"];
        match key {
            "features" => {
                // Native's whole-table assignment removes earlier sibling keys.
                observed = ObservedNativeFeatureControls {
                    hooks_enabled: value.get("hooks").and_then(toml::Value::as_bool),
                    request_compression_enabled: value
                        .get("enable_request_compression")
                        .and_then(toml::Value::as_bool),
                    network_proxy_enabled: value
                        .get("network_proxy")
                        .and_then(|proxy| proxy.get("enabled"))
                        .and_then(toml::Value::as_bool),
                };
            }
            "features.hooks" => observed.hooks_enabled = value.as_bool(),
            "features.enable_request_compression" => {
                observed.request_compression_enabled = value.as_bool()
            }
            "features.network_proxy" => {
                if let Some(enabled) = value.get("enabled") {
                    observed.network_proxy_enabled = enabled.as_bool();
                }
            }
            "features.network_proxy.enabled" => observed.network_proxy_enabled = value.as_bool(),
            _ => {}
        }
    }
    observed
}

#[test]
fn full_native_argument_order_preserves_owned_hooks_and_merged_profile_controls()
-> Result<(), Box<dyn std::error::Error>> {
    // Arrange: assemble the actual example's helper + spec argv in production order.
    let run = TestRunDirectory::create()?;
    let profile = luna_profile_from_text(&debug_profile("http://127.0.0.1:18787/v1"), 28787)?;
    let paths = CodexPaths::from_codex_home("/unused-native-home".into());
    let spec = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(28787),
        &run.0.join("native-socket/app-server.sock"),
    )
    .with_debug_profile(&profile);
    let mut arguments = vec![OsString::from("-c"), OsString::from("features.hooks=false")];
    arguments.extend(proof_permissions::automation_proof_startup_arguments(
        &run.0,
    )?);
    arguments.extend(spec.arguments());

    // Act / Assert: independent literal controls, not the renderer's own output.
    let expected = ObservedNativeFeatureControls {
        hooks_enabled: Some(false),
        request_compression_enabled: Some(false),
        network_proxy_enabled: Some(false),
    };
    assert_eq!(
        observe_ordered_native_feature_controls(&arguments),
        expected
    );
    assert!(!arguments.iter().any(|argument| {
        argument
            .to_str()
            .is_some_and(|value| value.starts_with("features="))
    }));

    // The exact old whole-features shape destroys the owned hooks/proxy controls.
    let mut legacy = arguments;
    legacy.extend([
        OsString::from("-c"),
        OsString::from("features={enable_request_compression=false}"),
    ]);
    let broken = observe_ordered_native_feature_controls(&legacy);
    assert_ne!(broken, expected);
    assert_eq!(broken.hooks_enabled, None);
    assert_eq!(broken.network_proxy_enabled, None);
    Ok(())
}
