use codex_native_integration::ResumeModelChoice;
use codex_native_integration::caller_overrides;
use std::ffi::OsString;
use std::path::PathBuf;

use codex_native_integration::AppServerCommandSpec;
use codex_native_integration::CodexPaths;
use codex_native_integration::CodexRouterProfile;
use codex_native_integration::SessionLaunch;
use codex_native_integration::profile_remote_resume_permission_keys;

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

fn expected_router_root_overrides() -> Vec<String> {
    vec![
        "model_provider=\"codex-router\"".to_owned(),
        "features.enable_request_compression=false".to_owned(),
        "model_providers.codex-router.name=\"OpenAI\"".to_owned(),
        "model_providers.codex-router.base_url=\"http://127.0.0.1:8787/v1\"".to_owned(),
        "model_providers.codex-router.wire_api=\"responses\"".to_owned(),
        "model_providers.codex-router.requires_openai_auth=true".to_owned(),
        "model_providers.codex-router.supports_websockets=true".to_owned(),
        "sandbox_mode=\"workspace-write\"".to_owned(),
        "features.network_proxy.enabled=false".to_owned(),
        "permissions.router-write-restricted.extends=\":read-only\"".to_owned(),
        "permissions.router-write-restricted.network.enabled=true".to_owned(),
        "permissions.router-workspace-write.extends=\":workspace\"".to_owned(),
        "permissions.router-workspace-write.network.enabled=true".to_owned(),
    ]
}

#[test]
fn rendered_profile_sets_no_permission_key_so_remote_resume_works() {
    // Act
    let keys =
        profile_remote_resume_permission_keys(&CodexRouterProfile::new(8787).render()).unwrap();

    // Assert
    assert!(keys.is_empty(), "profile must not set {keys:?}");
}

#[test]
fn router_profile_has_one_rendering_and_root_override_projection() {
    // Arrange: the production loopback port and the production collaboration directory.
    let profile = CodexRouterProfile::new(8787);

    // Act & assert: the profile file stays model routing only.
    assert_eq!(
        profile.render(),
        concat!(
            "model_provider = \"codex-router\"\n\n",
            "[model_providers.codex-router]\n",
            "name = \"OpenAI\"\n",
            "base_url = \"http://127.0.0.1:8787/v1\"\n",
            "wire_api = \"responses\"\n",
            "requires_openai_auth = true\n",
            "supports_websockets = true\n\n",
            "[features]\n",
            "enable_request_compression = false\n",
        )
    );
    // Assert: the managed child also carries both Router profiles' direct network.
    assert_eq!(profile.root_overrides(), expected_router_root_overrides());
}

#[test]
fn router_root_overrides_enable_direct_network_without_the_managed_proxy() {
    // Arrange: overrides are separate -c values; native merges them into one table.
    let overrides = CodexRouterProfile::new(8787).root_overrides();

    // Act: parse them exactly as TOML, proving the dotted keys are valid.
    let document = toml::Value::Table(overrides.join("\n").parse::<toml::Table>().unwrap());

    // Assert: Router owns the app-server's default sandbox, so starting it does not
    // depend on the owner's home configuration.
    assert_eq!(
        document.get("sandbox_mode").and_then(toml::Value::as_str),
        Some("workspace-write")
    );
    // Assert: the proxy is off, so Seatbelt allows direct DNS, ssh and local sockets.
    let proxy = document["features"]["network_proxy"].as_table().unwrap();
    assert_eq!(proxy.len(), 1);
    assert_eq!(
        proxy.get("enabled").and_then(toml::Value::as_bool),
        Some(false)
    );
    // Assert: each profile enables network with no proxy-only domain or socket rules.
    for profile in ["router-write-restricted", "router-workspace-write"] {
        let network = document["permissions"][profile]["network"]
            .as_table()
            .unwrap();
        assert_eq!(network.len(), 1, "{profile} network carries only enabled");
        assert_eq!(
            network.get("enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
        assert!(
            document["permissions"][profile].get("filesystem").is_none(),
            "{profile} filesystem belongs to per-session overrides"
        );
    }
}

#[test]
fn app_server_command_uses_managed_executable_profile_and_native_contract() {
    // Arrange.
    let paths = CodexPaths::from_codex_home(PathBuf::from("/Users/owner/.codex"));
    let socket = paths.app_server_socket();

    // Act.
    let command = AppServerCommandSpec::new(&paths, &CodexRouterProfile::new(8787), &socket);

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

#[test]
fn emitted_file_and_actual_argv_preserve_native_compaction_capability() {
    for port in [8787, 18787] {
        let profile = CodexRouterProfile::new(port);
        let rendered = profile.render().parse::<toml::Table>().unwrap();
        let paths = CodexPaths::from_codex_home("/unused-native-home".into());
        let command = AppServerCommandSpec::new(&paths, &profile, &paths.app_server_socket());
        let arguments = command.arguments();
        let overrides: Vec<&str> = arguments
            .windows(2)
            .filter(|pair| pair[0] == "-c")
            .map(|pair| pair[1].to_str().unwrap())
            .collect();
        let actual = overrides.join("\n").parse::<toml::Table>().unwrap();
        for configuration in [rendered, actual] {
            assert_eq!(
                configuration["model_provider"].as_str(),
                Some("codex-router")
            );
            let provider = &configuration["model_providers"]["codex-router"];
            assert_eq!(provider["name"].as_str(), Some("OpenAI"));
            assert_eq!(
                provider["base_url"].as_str(),
                Some(format!("http://127.0.0.1:{port}/v1").as_str())
            );
            assert_eq!(provider["wire_api"].as_str(), Some("responses"));
            assert_eq!(provider["requires_openai_auth"].as_bool(), Some(true));
            assert_eq!(provider["supports_websockets"].as_bool(), Some(true));
            assert_eq!(
                configuration["features"]["enable_request_compression"].as_bool(),
                Some(false)
            );
        }
    }
}
