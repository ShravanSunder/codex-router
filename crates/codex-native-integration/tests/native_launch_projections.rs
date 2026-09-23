use codex_native_integration::ResumeModelChoice;
use codex_native_integration::caller_overrides;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

use codex_native_integration::AppServerCommandSpec;
use codex_native_integration::CodexPaths;
use codex_native_integration::CodexRouterProfile;
use codex_native_integration::RouterControlSocketPath;
use codex_native_integration::SessionLaunch;

#[test]
fn codex_paths_keep_native_state_under_normal_codex_home() {
    let paths = CodexPaths::from_codex_home(PathBuf::from("/Users/owner/.codex"));

    assert_eq!(
        paths.app_server_socket(),
        PathBuf::from("/Users/owner/.codex/app-server-control/app-server-control.sock")
    );
    assert_eq!(
        paths.managed_executable(),
        PathBuf::from("/Users/owner/.codex/packages/standalone/current/codex")
    );
}

const CONTROL_SOCKET: &str = "/Users/owner/.codex-router/agent-communication/control.sock";

const COLLABORATION_DIRECTORY: &str = "/Users/owner/.codex-router/agent-communication";

fn expected_router_root_overrides() -> Vec<String> {
    vec![
        "model_provider=\"codex-router\"".to_owned(),
        "model_providers.codex-router.name=\"codex-router\"".to_owned(),
        "model_providers.codex-router.base_url=\"http://127.0.0.1:8787/v1\"".to_owned(),
        "model_providers.codex-router.wire_api=\"responses\"".to_owned(),
        "model_providers.codex-router.requires_openai_auth=true".to_owned(),
        "model_providers.codex-router.supports_websockets=true".to_owned(),
        "features.network_proxy.enabled=true".to_owned(),
        "features.network_proxy.mode=\"full\"".to_owned(),
        "features.network_proxy.domains={\"*\"=\"allow\"}".to_owned(),
        format!("features.network_proxy.unix_sockets={{\"{CONTROL_SOCKET}\"=\"allow\"}}"),
        "features.network_proxy.allow_local_binding=false".to_owned(),
        "features.network_proxy.dangerously_allow_all_unix_sockets=false".to_owned(),
        "features.network_proxy.enable_socks5=false".to_owned(),
        "features.network_proxy.allow_upstream_proxy=false".to_owned(),
        "features.network_proxy.credential_broker=false".to_owned(),
        "permissions.router-write-restricted.extends=\":read-only\"".to_owned(),
        "permissions.router-write-restricted.network.enabled=true".to_owned(),
        "permissions.router-write-restricted.network.mode=\"full\"".to_owned(),
        "permissions.router-write-restricted.network.domains={\"*\"=\"allow\"}".to_owned(),
        format!(
            "permissions.router-write-restricted.network.unix_sockets={{\"{CONTROL_SOCKET}\"=\"allow\"}}"
        ),
        "permissions.router-workspace-write.extends=\":workspace\"".to_owned(),
        "permissions.router-workspace-write.network.enabled=true".to_owned(),
        "permissions.router-workspace-write.network.mode=\"full\"".to_owned(),
        "permissions.router-workspace-write.network.domains={\"*\"=\"allow\"}".to_owned(),
        format!(
            "permissions.router-workspace-write.network.unix_sockets={{\"{CONTROL_SOCKET}\"=\"allow\"}}"
        ),
    ]
}

#[test]
fn router_profile_has_one_rendering_and_root_override_projection() {
    // Arrange: the production loopback port and the production collaboration directory.
    let profile = CodexRouterProfile::new(8787);
    let owner_control_socket =
        RouterControlSocketPath::in_collaboration_directory(Path::new(COLLABORATION_DIRECTORY))
            .unwrap();

    // Act & assert: the profile file stays model routing only.
    assert_eq!(
        profile.render(),
        concat!(
            "model_provider = \"codex-router\"\n\n",
            "[model_providers.codex-router]\n",
            "name = \"codex-router\"\n",
            "base_url = \"http://127.0.0.1:8787/v1\"\n",
            "wire_api = \"responses\"\n",
            "requires_openai_auth = true\n",
            "supports_websockets = true\n",
        )
    );
    // Assert: the managed child also carries the Router socket network profile.
    assert_eq!(
        profile.root_overrides(&owner_control_socket),
        expected_router_root_overrides()
    );
}

#[test]
fn router_root_overrides_allow_every_host_and_only_the_control_socket() {
    // Arrange: overrides are separate -c values; native merges them into one table.
    let owner_control_socket =
        RouterControlSocketPath::in_collaboration_directory(Path::new(COLLABORATION_DIRECTORY))
            .unwrap();
    let overrides = CodexRouterProfile::new(8787).root_overrides(&owner_control_socket);

    // Act: parse them exactly as TOML, proving the inline tables and dotted keys are valid.
    let document = toml::Value::Table(overrides.join("\n").parse::<toml::Table>().unwrap());

    // Assert: one allowed socket, no socket-wide escape hatch, no local binding.
    let networks = [
        &document["features"]["network_proxy"],
        &document["permissions"]["router-write-restricted"]["network"],
        &document["permissions"]["router-workspace-write"]["network"],
    ];
    for network in networks {
        assert_eq!(
            network.get("enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert_eq!(
            network.get("mode").and_then(toml::Value::as_str),
            Some("full")
        );
        let domains = network
            .get("domains")
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(domains.len(), 1);
        assert_eq!(
            domains.get("*").and_then(toml::Value::as_str),
            Some("allow")
        );
        let sockets = network
            .get("unix_sockets")
            .and_then(toml::Value::as_table)
            .unwrap();
        assert_eq!(sockets.len(), 1);
        assert_eq!(
            sockets.get(CONTROL_SOCKET).and_then(toml::Value::as_str),
            Some("allow")
        );
    }
    let proxy = &document["features"]["network_proxy"];
    for denied in [
        "allow_local_binding",
        "dangerously_allow_all_unix_sockets",
        "enable_socks5",
        "allow_upstream_proxy",
        "credential_broker",
    ] {
        assert_eq!(
            proxy.get(denied).and_then(toml::Value::as_bool),
            Some(false),
            "{denied} must stay off"
        );
    }
}

#[test]
fn router_control_socket_requires_an_absolute_collaboration_directory() {
    // Arrange & act: a relative directory cannot name a canonical socket.
    let socket =
        RouterControlSocketPath::in_collaboration_directory(Path::new("agent-communication"));

    // Assert.
    assert!(socket.is_err());
    assert_eq!(
        RouterControlSocketPath::in_collaboration_directory(Path::new(COLLABORATION_DIRECTORY))
            .unwrap()
            .as_path(),
        Path::new(CONTROL_SOCKET)
    );
}

#[test]
fn app_server_command_uses_managed_executable_profile_and_native_contract() {
    // Arrange.
    let paths = CodexPaths::from_codex_home(PathBuf::from("/Users/owner/.codex"));
    let socket = paths.app_server_socket();
    let owner_control_socket =
        RouterControlSocketPath::in_collaboration_directory(Path::new(COLLABORATION_DIRECTORY))
            .unwrap();

    // Act.
    let command = AppServerCommandSpec::new(
        &paths,
        &CodexRouterProfile::new(8787),
        &owner_control_socket,
        &socket,
    );

    // Assert: every root override reaches the child as its own -c argument.
    assert_eq!(command.executable(), paths.managed_executable());
    let mut expected: Vec<OsString> = expected_router_root_overrides()
        .into_iter()
        .flat_map(|root_override| [OsString::from("-c"), OsString::from(root_override)])
        .collect();
    expected.extend([
        OsString::from("app-server"),
        OsString::from("--remote-control"),
        OsString::from("--listen"),
        OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
    ]);
    assert_eq!(command.arguments(), expected);
}

#[test]
fn session_launch_keeps_remote_at_root_for_new_and_resume() {
    let socket = PathBuf::from("/Users/owner/.codex/app-server-control/app-server-control.sock");
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let user_arguments = vec![OsString::from("--model"), OsString::from("gpt-5.4")];

    assert_eq!(
        SessionLaunch::new(&socket, &invoking_cwd, &user_arguments).arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.4"),
        ]
    );
    assert_eq!(
        SessionLaunch::resume(
            &socket,
            &invoking_cwd,
            &user_arguments,
            "thread_123",
            &ResumeModelChoice::default()
        )
        .arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.4"),
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
    assert_eq!(
        SessionLaunch::fork(
            &socket,
            &invoking_cwd,
            &user_arguments,
            "thread_123",
            &ResumeModelChoice::default()
        )
        .arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.4"),
            OsString::from("fork"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
}

#[test]
fn local_session_launch_keeps_router_profile_without_remote_attachment() {
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let user_arguments = vec![
        OsString::from("--model"),
        OsString::from("gpt-5.6-luna"),
        OsString::from("--yolo"),
    ];

    assert_eq!(
        SessionLaunch::local(&invoking_cwd, &user_arguments).arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--yolo"),
        ]
    );
    assert_eq!(
        SessionLaunch::resume_local(
            &invoking_cwd,
            &user_arguments,
            "thread_123",
            &ResumeModelChoice::default()
        )
        .arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--yolo"),
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
    assert_eq!(
        SessionLaunch::fork_local(
            &invoking_cwd,
            &user_arguments,
            "thread_123",
            &ResumeModelChoice::default()
        )
        .arguments(),
        vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--cd"),
            OsString::from("/Users/owner/project"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--yolo"),
            OsString::from("fork"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]
    );
}

#[test]
fn session_launch_preserves_every_explicit_cwd_spelling_without_injecting_a_duplicate() {
    let socket = PathBuf::from("/Users/owner/.codex/app-server-control/app-server-control.sock");
    let invoking_cwd = PathBuf::from("/Users/owner/invoking-project");
    let explicit_cwd_spellings = [
        vec!["--cd", "/Users/owner/explicit-project"],
        vec!["--cd=/Users/owner/explicit-project"],
        vec!["-C", "/Users/owner/explicit-project"],
        vec!["-Cexplicit-project"],
    ];

    for explicit_cwd_spelling in explicit_cwd_spellings {
        let user_arguments = explicit_cwd_spelling
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        let mut expected_arguments = vec![
            OsString::from("--profile"),
            OsString::from("codex-router"),
            OsString::from("--remote"),
            OsString::from("unix:///Users/owner/.codex/app-server-control/app-server-control.sock"),
        ];
        expected_arguments.extend(user_arguments.iter().cloned());
        expected_arguments.extend([
            OsString::from("resume"),
            OsString::from("--"),
            OsString::from("thread_123"),
        ]);

        assert_eq!(
            SessionLaunch::resume(
                &socket,
                &invoking_cwd,
                &user_arguments,
                "thread_123",
                &ResumeModelChoice::default()
            )
            .arguments(),
            expected_arguments,
        );
    }
}

fn text_arguments(arguments: &[OsString]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect()
}

#[test]
fn caller_overrides_detect_every_model_and_effort_argument_form() {
    // Arrange / Act / Assert: each row is one argument vector and the keys it claims.
    let cases: Vec<(Vec<&str>, bool, bool)> = vec![
        (vec![], false, false),
        (vec!["--search"], false, false),
        (vec!["-m", "gpt-6"], true, false),
        (vec!["--model", "gpt-6"], true, false),
        (vec!["-m=gpt-6"], true, false),
        (vec!["-mgpt-6"], true, false),
        (vec!["--model=gpt-6"], true, false),
        (vec!["-c", "model=\"gpt-6\""], true, false),
        (vec!["-cmodel=\"gpt-6\""], true, false),
        (vec!["--config", "model=\"gpt-6\""], true, false),
        (vec!["--config=model=\"gpt-6\""], true, false),
        (vec!["-c", "model_reasoning_effort=\"high\""], false, true),
        (
            vec!["--config", "model_reasoning_effort=\"high\""],
            false,
            true,
        ),
        (
            vec!["--config=model_reasoning_effort=\"high\""],
            false,
            true,
        ),
        (vec!["-c", "model_verbosity=\"high\""], false, false),
        (
            vec!["-m", "gpt-6", "-c", "model_reasoning_effort=\"high\""],
            true,
            true,
        ),
    ];

    for (arguments, expects_model, expects_effort) in cases {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        let overrides = caller_overrides(&arguments);
        assert_eq!(
            (overrides.model, overrides.reasoning_effort),
            (expects_model, expects_effort),
            "unexpected overrides for {arguments:?}"
        );
    }
}

#[test]
fn resume_injects_stored_model_and_effort_immediately_before_the_subcommand() {
    // Arrange
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let user_arguments = vec![OsString::from("--search")];
    let choice = ResumeModelChoice::from_stored_values(Some("gpt-6-astra"), Some("high"));

    // Act
    let resume = SessionLaunch::resume_local(&invoking_cwd, &user_arguments, "thread_123", &choice);
    let fork = SessionLaunch::fork_local(&invoking_cwd, &user_arguments, "thread_123", &choice);

    // Assert
    assert_eq!(
        text_arguments(&resume.arguments()),
        vec![
            "--profile",
            "codex-router",
            "--cd",
            "/Users/owner/project",
            "--search",
            "-c",
            "model=\"gpt-6-astra\"",
            "-c",
            "model_reasoning_effort=\"high\"",
            "resume",
            "--",
            "thread_123",
        ]
    );
    assert_eq!(
        text_arguments(&fork.arguments())[4..9],
        [
            "--search",
            "-c",
            "model=\"gpt-6-astra\"",
            "-c",
            "model_reasoning_effort=\"high\"",
        ]
    );
}

#[test]
fn resume_leaves_arguments_unchanged_without_stored_model_or_effort() {
    // Arrange
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let user_arguments = vec![OsString::from("--search")];
    let empty = ResumeModelChoice::from_stored_values(None, None);

    // Act
    let launch = SessionLaunch::resume_local(&invoking_cwd, &user_arguments, "thread_123", &empty);

    // Assert
    assert_eq!(
        text_arguments(&launch.arguments()),
        vec![
            "--profile",
            "codex-router",
            "--cd",
            "/Users/owner/project",
            "--search",
            "resume",
            "--",
            "thread_123",
        ]
    );
}

#[test]
fn caller_model_argument_wins_over_the_stored_model() {
    // Arrange
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let user_arguments = vec![OsString::from("-m"), OsString::from("gpt-6-mini")];
    let choice = ResumeModelChoice::from_stored_values(Some("gpt-6-astra"), Some("high"));

    // Act
    let launch = SessionLaunch::resume_local(&invoking_cwd, &user_arguments, "thread_123", &choice);

    // Assert
    let arguments = text_arguments(&launch.arguments());
    assert!(
        !arguments
            .iter()
            .any(|argument| argument.starts_with("model="))
    );
    assert_eq!(
        arguments[4..8],
        ["-m", "gpt-6-mini", "-c", "model_reasoning_effort=\"high\""]
    );
}

#[test]
fn stored_values_that_cannot_be_quoted_are_dropped_from_the_launch() {
    // Arrange
    let invoking_cwd = PathBuf::from("/Users/owner/project");
    let unsafe_model = Some("gpt-6\"injected");

    // Act
    let choice = ResumeModelChoice::from_stored_values(unsafe_model, Some("high"));
    let launch = SessionLaunch::resume_local(&invoking_cwd, &[], "thread_123", &choice);

    // Assert
    assert!(ResumeModelChoice::rejects_stored_value(
        unsafe_model,
        Some("high")
    ));
    let arguments = text_arguments(&launch.arguments());
    assert!(
        !arguments
            .iter()
            .any(|argument| argument.starts_with("model="))
    );
    assert!(arguments.contains(&"model_reasoning_effort=\"high\"".to_owned()));
}
