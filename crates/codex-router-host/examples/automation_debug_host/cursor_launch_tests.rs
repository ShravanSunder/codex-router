//! Explicit Cursor launch input, admission and replacement binding for the acceptance example.
use super::*;

fn cursor_options() -> Result<launch_options::DebugHostOptions, Box<dyn std::error::Error>> {
    parse_options_from(
        [
            "--run-directory",
            "/tmp/cursor-proof",
            "--router-binary",
            "/tmp/router",
            "--router-root",
            "/tmp/cursor-proof-account",
            "--port",
            "28787",
            "--cursor-acp-executable",
            "/usr/bin/python3",
            "--cursor-acp-arguments",
            "-u",
            "--cursor-acp-arguments",
            "/tmp/scripted cursor.py",
            "--cursor-acp-arguments",
            r#"{"steps":["literal with spaces"]}"#,
        ]
        .map(OsString::from),
    )?
    .ok_or_else(|| "Cursor launch options missing".into())
}

fn cursor_host_config(
    options: &launch_options::DebugHostOptions,
) -> Result<HostConfig, Box<dyn std::error::Error>> {
    Ok(HostConfig::new(HostConfigInputs {
        coordination_paths: options.host_coordination_paths(Path::new("/tmp/cursor-proof-account")),
        router_endpoint: "127.0.0.1:28787".parse()?,
        mcp_bind: "127.0.0.1:0".parse()?,
        app_server_socket: "/tmp/native.sock".into(),
        managed_executable: "/tmp/codex".into(),
        deadlines: HostDeadlines::production(),
    }))
}

#[test]
fn cursor_parser_preserves_literal_command_and_repeated_arguments() {
    let options = cursor_options().expect("explicit isolated Cursor options");
    assert_eq!(
        options.cursor_provider,
        Some(ProviderCommand {
            executable: "/usr/bin/python3".into(),
            arguments: vec![
                "-u".into(),
                "/tmp/scripted cursor.py".into(),
                r#"{"steps":["literal with spaces"]}"#.into(),
            ],
        })
    );
    assert!(options.claude_provider.is_none());
    assert_eq!(
        options.router_root,
        Some("/tmp/cursor-proof-account".into())
    );
}

#[test]
fn cursor_parser_requires_explicit_absolute_root_and_executable() {
    for extras in [
        vec!["--cursor-acp-executable", "/absolute/acp"],
        vec![
            "--router-root",
            "relative/root",
            "--cursor-acp-executable",
            "/absolute/acp",
        ],
        vec![
            "--router-root",
            "/tmp/owned",
            "--cursor-acp-executable",
            "relative/acp",
        ],
        vec!["--cursor-acp-arguments", "-u"],
        vec!["--cursor-acp-executable"],
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

#[test]
fn cursor_parser_rejects_non_utf8_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let mut arguments = vec![
        "--run-directory",
        "/tmp/proof",
        "--router-binary",
        "/tmp/router",
        "--router-root",
        "/tmp/owned",
        "--cursor-acp-executable",
        "/absolute/acp",
        "--cursor-acp-arguments",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    arguments.push(OsString::from_vec(vec![0xff]));
    assert!(parse_options_from(arguments).is_err());
}

#[test]
fn cursor_is_not_launched_without_explicit_input() {
    let options = parse_options_from(
        [
            "--run-directory",
            "/tmp/proof",
            "--router-binary",
            "/tmp/router",
        ]
        .map(OsString::from),
    )
    .expect("default options")
    .expect("launch options");
    assert!(options.cursor_provider.is_none());
    assert!(options.claude_provider.is_none());
    let config = cursor_host_config(&options).expect("inert Host configuration");
    assert_eq!(
        options
            .attach_provider(config.clone(), None)
            .expect("no provider attachment"),
        config
    );
}

#[test]
fn cursor_attaches_existing_binding_without_a_claude_selector() {
    let options = cursor_options().expect("Cursor options");
    let configured = options
        .attach_provider(
            cursor_host_config(&options).expect("Host configuration"),
            None,
        )
        .expect("Cursor-only attachment");
    assert_eq!(configured.external_provider_startups().len(), 1);
    let binding = configured
        .external_provider_startups()
        .iter()
        .find_map(|startup| match startup {
            codex_router_host::ExternalProviderStartup::Launch(binding) => Some(binding),
            codex_router_host::ExternalProviderStartup::Unavailable { .. } => None,
        })
        .expect("actual launch binding");
    assert_eq!(String::from(binding.endpoint_id.clone()), "cursor-local");
    assert_eq!(binding.launch.executable, PathBuf::from("/usr/bin/python3"));
    assert_eq!(
        binding.launch.arguments,
        [
            "-u",
            "/tmp/scripted cursor.py",
            r#"{"steps":["literal with spaces"]}"#
        ]
    );
    assert!(binding.launch.environment.is_empty());
}

#[test]
fn cursor_attachment_preserves_the_existing_claude_command_and_selector() {
    let mut options = isolated_acp_options();
    options.cursor_provider = cursor_options().expect("Cursor options").cursor_provider;
    let configured = options
        .attach_provider(
            cursor_host_config(&options).expect("Host configuration"),
            Some("/global/claude".into()),
        )
        .expect("both explicit providers");
    assert_eq!(configured.external_provider_startups().len(), 2);
    let bindings = configured
        .external_provider_startups()
        .iter()
        .filter_map(|startup| match startup {
            codex_router_host::ExternalProviderStartup::Launch(binding) => Some(binding),
            codex_router_host::ExternalProviderStartup::Unavailable { .. } => None,
        })
        .collect::<Vec<_>>();
    let claude = bindings.first().expect("Claude first");
    assert_eq!(String::from(claude.endpoint_id.clone()), "claude-local");
    assert_eq!(
        claude.launch.executable,
        PathBuf::from("/opt/homebrew/bin/pnpm")
    );
    assert_eq!(
        claude.launch.arguments,
        ["dlx", "@agentclientprotocol/claude-agent-acp@0.81.2"]
    );
    assert_eq!(
        claude.launch.environment,
        [(
            "CLAUDE_CODE_EXECUTABLE".to_owned(),
            "/global/claude".to_owned()
        )]
    );
    let cursor = bindings.get(1).expect("Cursor second");
    assert_eq!(String::from(cursor.endpoint_id.clone()), "cursor-local");
    assert!(cursor.launch.environment.is_empty());
}

#[test]
fn cursor_replacement_preserves_exact_argv_without_shell_rewriting() {
    let options = cursor_options().expect("Cursor options");
    let expected = [
        "--resume-run-directory",
        "/tmp/cursor-proof",
        "--router-binary",
        "/tmp/router",
        "--port",
        "28787",
        "--router-root",
        "/tmp/cursor-proof-account",
        "--cursor-acp-executable",
        "/usr/bin/python3",
        "--cursor-acp-arguments",
        "-u",
        "--cursor-acp-arguments",
        "/tmp/scripted cursor.py",
        "--cursor-acp-arguments",
        r#"{"steps":["literal with spaces"]}"#,
    ]
    .map(OsString::from)
    .to_vec();
    assert_eq!(options.resume_arguments(), expected);
    let resumed = parse_options_from(expected.clone())
        .expect("resume arguments")
        .expect("resume launch");
    assert_eq!(resumed.cursor_provider, options.cursor_provider);
    assert_eq!(
        resumed.run_directory_admission,
        RunDirectoryAdmission::Resume
    );
    assert_eq!(
        host_replacement::build_host_replacement_command(&options).expect("replacement command"),
        ChildCommandSpec::new(std::env::current_exe().expect("test executable"))
            .with_arguments(expected)
            .with_output(ChildOutput::Telemetry)
    );
}

#[test]
fn cursor_private_context_round_trip_rejects_changed_resume_input() {
    let options = cursor_options().expect("Cursor options");
    let run = TestRunDirectory::create().expect("owned private test directory");
    run.write_context(stopped_process_id().expect("stopped fixture process"))
        .expect("initial context");
    let mut context = read_existing_context(&run.0).expect("initial private context");
    context.router_root = options.router_root.clone();
    assert!(context.cursor_provider.is_none());
    assert!(validate_resume_binding(&context, &options).is_err());
    context.cursor_provider = options.cursor_provider.clone();
    replace_resumed_context(&context).expect("retain explicit Cursor command");
    let context = read_existing_context(&run.0).expect("read retained Cursor command");
    assert_eq!(context.cursor_provider, options.cursor_provider);
    validate_resume_binding(&context, &options).expect("identical command admitted");

    let mut removed = options.clone();
    removed.cursor_provider = None;
    assert!(validate_resume_binding(&context, &removed).is_err());
    let mut changed_executable = options.clone();
    changed_executable
        .cursor_provider
        .as_mut()
        .expect("Cursor command")
        .executable = "/another/python3".into();
    assert!(validate_resume_binding(&context, &changed_executable).is_err());
    let mut changed_arguments = options.clone();
    changed_arguments
        .cursor_provider
        .as_mut()
        .expect("Cursor command")
        .arguments
        .reverse();
    assert!(validate_resume_binding(&context, &changed_arguments).is_err());
}
