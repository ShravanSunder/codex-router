use super::*;

#[test]
fn host_commands_parse_with_explicit_router_root() {
    let router_root = PathBuf::from("/tmp/router-host-test");
    let command = match CliCommand::parse([
        OsString::from("codex-router"),
        OsString::from("host"),
        OsString::from("status"),
        OsString::from("--router-root"),
        router_root.as_os_str().to_owned(),
    ]) {
        Ok(CliCommand::Host(command)) => command,
        Ok(other) => panic!("host status should parse, got {other:?}"),
        Err(error) => panic!("host status should parse: {error}"),
    };

    assert_eq!(command.action(), crate::host_command::HostAction::Status);
    assert_eq!(command.router_root(), Some(router_root.as_path()));
}

#[test]
fn development_router_root_defaults_to_home_debug_root() {
    let home = PathBuf::from("/tmp/codex-router-dev-home");

    assert_eq!(
        super::default_router_root_from_environment(
            Some(home.clone().into_os_string()),
            None,
            None,
            true
        )
        .unwrap_or_else(|error| panic!("debug default root should resolve: {error}")),
        home.join(".codex-router-debug")
    );
}

#[test]
fn development_router_root_honors_debug_override_without_home() {
    let debug_root = PathBuf::from("/tmp/codex-router-explicit-debug-root");

    assert_eq!(
        super::default_router_root_from_environment(
            None,
            Some(debug_root.clone().into_os_string()),
            None,
            true,
        )
        .unwrap_or_else(|error| panic!("debug override should resolve: {error}")),
        debug_root
    );
}

#[test]
fn development_router_root_home_default_escape_uses_prod_root() {
    let home = PathBuf::from("/tmp/codex-router-dev-home");
    let debug_root = PathBuf::from("/tmp/codex-router-explicit-debug-root");

    assert_eq!(
        super::default_router_root_from_environment(
            Some(home.clone().into_os_string()),
            Some(debug_root.into_os_string()),
            Some(OsString::from("1")),
            true,
        )
        .unwrap_or_else(|error| panic!("home-default escape should resolve: {error}")),
        home.join(".codex-router")
    );
}

#[test]
fn debug_app_server_socket_requires_absolute_dedicated_parent() {
    assert!(
        super::validate_debug_app_server_socket(PathBuf::from("relative/app-server.sock")).is_err()
    );
    assert!(
        super::validate_debug_app_server_socket(std::env::temp_dir().join("app-server.sock"))
            .is_err()
    );
}

#[test]
fn debug_app_server_socket_accepts_absolute_dedicated_parent() {
    let socket = std::env::temp_dir()
        .join("codex-router-debug-app-server")
        .join("app-server.sock");

    assert_eq!(
        super::validate_debug_app_server_socket(socket.clone()),
        Ok(socket)
    );
}

#[test]
fn explicit_router_root_option_wins_over_defaults() {
    let explicit_router_root = PathBuf::from("/tmp/codex-router-explicit-root");
    let command = match CliCommand::parse([
        OsString::from("account"),
        OsString::from("list"),
        OsString::from("--router-root"),
        explicit_router_root.clone().into_os_string(),
    ]) {
        Ok(CliCommand::Account(command)) => command,
        Ok(other) => panic!("account command should parse, got {other:?}"),
        Err(error) => panic!("account command should parse: {error}"),
    };

    let AccountCommand::List { router_root } = command else {
        panic!("account list command should parse");
    };
    assert_eq!(router_root, explicit_router_root);
}

#[test]
fn reports_package_name() {
    assert_eq!(package_name(), "codex-router-cli");
}

#[test]
fn websocket_registry_report_value_omits_raw_session_ids_and_peer_addrs() {
    let snapshot = WebSocketRegistrySnapshot {
        active_sessions: 0,
        high_water_sessions: 3,
        registered_sessions: 3,
        closed_sessions: 3,
        completed_response_sessions: 2,
        forwarded_upstream_messages: 9,
        registered_session_ids: vec![1, 2, 3],
        completed_session_ids: vec![2, 3],
        closed_session_ids: vec![1, 2, 3],
        session_peer_addrs: vec![WebSocketSessionPeerAddr {
            session_id: 1,
            peer_addr: "127.0.0.1:61234".to_owned(),
        }],
        completed_session_forwarded_upstream_message_counts: vec![3, 4],
        final_session_forwarded_upstream_message_counts: vec![3, 3, 3],
        quota_reconnect_signal_count: 1,
        quota_reconnect_signal_unix_ms: Some(1_720_000_000_000),
    };

    let report = websocket_registry_report_value(3, &snapshot);
    let rendered = serde_json::to_string(&report)
        .unwrap_or_else(|error| panic!("registry report should render: {error}"));

    assert_eq!(report["schema_version"], 2);
    assert_eq!(
        report["websocket_registry"]["registered_session_id_count"],
        3
    );
    assert_eq!(
        report["websocket_registry"]["completed_session_id_count"],
        2
    );
    assert_eq!(report["websocket_registry"]["closed_session_id_count"], 3);
    assert_eq!(report["websocket_registry"]["session_peer_addr_count"], 1);
    assert_eq!(
        report["websocket_registry"]["session_peer_join_observable"],
        true
    );
    for forbidden_key in [
        "registered_session_ids",
        "completed_session_ids",
        "closed_session_ids",
        "session_peer_addrs",
        "session_id",
        "peer_addr",
    ] {
        assert!(
            !rendered.contains(&format!("\"{forbidden_key}\"")),
            "persisted registry report leaked raw key {forbidden_key}"
        );
    }
    assert!(
        !rendered.contains("127.0.0.1:61234"),
        "persisted registry report leaked raw loopback peer address"
    );
}

#[test]
fn process_binary_path_supports_top_level_version() {
    let output = run_cli(
        ["/tmp/build/target/debug/codex-router", "--version"],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    assert_eq!(
        output.stdout,
        format!("codex-router {}\n", env!("CARGO_PKG_VERSION"))
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn process_binary_path_is_skipped_before_command_parse() {
    let output = run_cli(
        ["/tmp/build/target/debug/codex-router", "--help"],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    for expected_line in [
        "serve                         Run the local Codex account router",
        "account login --label <name>  Add an OAuth account",
        "account list                  Show configured router accounts",
        "account set-weekly-floor      Set or disable an account weekly quota floor",
        "quota                         Show quota, refresh state, and next account",
        "doctor                        Diagnose local router setup",
        "Sessions and agent communication: run agent-sessions --help.",
    ] {
        assert!(
            output.stdout.contains(expected_line),
            "help output missing expected line: {expected_line}\n{}",
            output.stdout
        );
    }
    for hidden_line in [
        "--state-db",
        "--secret-root",
        "--upstream-base-url",
        "serve internal proof flags",
        "token",
        "account import-codex-auth",
        "profile write",
        "live quota",
        "--scope",
        "--router-root",
        "--allow-plaintext-file-secrets",
    ] {
        assert!(
            !output.stdout.contains(hidden_line),
            "help output leaked internal line: {hidden_line}\n{}",
            output.stdout
        );
    }
    assert!(output.stderr.is_empty());
}

#[test]
fn nested_user_facing_help_does_not_leak_internal_commands() {
    for (arguments, expected_lines) in [
        (
            &["codex-router", "account", "--help"][..],
            &[
                "codex-router account",
                "login --label <name>  Add an OAuth account",
                "list                  Show configured router accounts",
            ][..],
        ),
        (
            &["codex-router", "account", "login", "--help"][..],
            &[
                "codex-router account login --label <name>",
                "--label <name>",
                "--codex-bin <path>",
            ][..],
        ),
        (
            &["codex-router", "quota", "--help"][..],
            &[
                "codex-router quota",
                "quota          Show persisted quota status and next account",
                "quota refresh  Refresh quota data now",
            ][..],
        ),
        (
            &["codex-router", "quota", "refresh", "--help"][..],
            &[
                "codex-router quota refresh",
                "Refreshes persisted quota data from configured OAuth accounts.",
            ][..],
        ),
        (
            &["codex-router", "quota", "reset", "--help"][..],
            &[
                "codex-router quota reset",
                "Quota reset moved to codex-router quota",
                "focus an account and press Ctrl-R",
            ][..],
        ),
    ] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        must_ok(test_async_runtime().block_on(run_with_io_async(
            arguments.iter().map(OsString::from).collect(),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        )));
        let output = CliRunOutput {
            stdout: must_ok(String::from_utf8(stdout)),
            stderr: must_ok(String::from_utf8(stderr)),
        };

        for expected_line in expected_lines {
            assert!(
                output.stdout.contains(expected_line),
                "help output missing expected line: {expected_line}\n{}",
                output.stdout
            );
        }
        for hidden_line in [
            "--state-db",
            "--secret-root",
            "token",
            "import-codex-auth",
            "live quota",
            "--allow-plaintext-file-secrets",
        ] {
            assert!(
                !output.stdout.contains(hidden_line),
                "help output leaked internal line: {hidden_line}\n{}",
                output.stdout
            );
        }
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn doctor_reports_stale_and_missing_state_without_secrets() {
    let report = DoctorReport::new(vec![
        DoctorAccountState::new(
            "primary",
            QuotaDoctorState::Stale {
                age_seconds: 600,
                secret_canary: "refresh-token-canary".to_owned(),
            },
        ),
        DoctorAccountState::new("secondary", QuotaDoctorState::Missing),
    ]);

    let rendered = report.render();

    assert!(rendered.contains("primary"));
    assert!(rendered.contains("quota: stale age=600s"));
    assert!(rendered.contains("secondary"));
    assert!(rendered.contains("quota: missing"));
    assert!(!rendered.contains("refresh-token-canary"));
    assert!(!rendered.contains("token"));
}

#[test]
fn quota_command_defaults_to_status() {
    let command = match CliCommand::parse([OsString::from("quota")]) {
        Ok(CliCommand::Quota(command)) => command,
        Ok(other) => panic!("quota command should parse, got {other:?}"),
        Err(error) => panic!("quota command should parse: {error}"),
    };

    let QuotaCommand::Status { router_root, .. } = command else {
        panic!("bare quota command should parse as status");
    };
    assert_eq!(router_root, default_router_root_for_test());
}

#[test]
fn quota_command_treats_leading_options_as_status_options() {
    let command = match CliCommand::parse([
        OsString::from("quota"),
        OsString::from("--format"),
        OsString::from("json"),
        OsString::from("--now-unix-seconds"),
        OsString::from("0"),
    ]) {
        Ok(CliCommand::Quota(command)) => command,
        Ok(other) => panic!("quota command should parse, got {other:?}"),
        Err(error) => panic!("quota command should parse: {error}"),
    };

    let QuotaCommand::Status {
        format,
        now_unix_seconds,
        ..
    } = command
    else {
        panic!("quota options should parse as status options");
    };
    assert_eq!(format, crate::quota::QuotaStatusFormat::Json);
    assert_eq!(now_unix_seconds, 0);
}

#[test]
fn terminal_ui_dependency_contract_uses_iocraft_presentation_boundary() {
    let workspace_manifest = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| panic!("cli crate should have workspace root parent"))
            .join("Cargo.toml"),
    ));
    let cli_manifest = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    ));

    assert!(workspace_manifest.contains("iocraft = "));
    assert!(cli_manifest.contains("iocraft.workspace = true"));
    assert!(cli_manifest.contains("comfy-table.workspace = true"));
    for disallowed_dependency in ["inquire", "ratatui", "dialoguer"] {
        assert!(
            !cli_manifest.contains(&format!("{disallowed_dependency}.workspace"))
                && !cli_manifest.contains(&format!("{disallowed_dependency} =")),
            "terminal UI must not add direct {disallowed_dependency} dependency"
        );
    }

    let presentation_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/presentation");
    let terminal_ui_sources = must_ok(fs::read_dir(&presentation_root))
        .map(|entry| must_ok(entry).path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .map(|path| must_ok(fs::read_to_string(path)))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        terminal_ui_sources.contains("iocraft"),
        "iocraft usage should be isolated inside the CLI presentation layer"
    );
}

#[test]
fn quota_reset_test_harness_is_a_distinct_non_installable_package() {
    let cli_manifest = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    ));
    let harness_manifest = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap_or_else(|| panic!("CLI crate should have workspace crates parent"))
            .join("codex-router-quota-reset-test-harness/Cargo.toml"),
    ));
    let installed_main = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"),
    ));
    let quota_source = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/quota.rs"),
    ));

    assert!(!cli_manifest.contains("name = \"codex-router-quota-reset-test-harness\""));
    assert!(!cli_manifest.contains("portable-pty"));
    assert!(cli_manifest.contains("quota-reset-test-harness = ["));
    assert!(cli_manifest.contains("agent-sessions/quota-reset-test-harness"));
    assert!(harness_manifest.contains("name = \"codex-router-quota-reset-test-harness\""));
    assert!(harness_manifest.contains("publish = false"));
    assert!(harness_manifest.contains("portable-pty"));
    assert!(!installed_main.contains("run_quota_reset_test_harness"));
    assert!(!quota_source.contains("quota-reset-test-harness"));
}

#[tokio::test]
async fn quota_reset_async_dispatch_prints_migration_without_state_or_network() {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run_with_io_async(
        vec![
            OsString::from("codex-router"),
            OsString::from("quota"),
            OsString::from("reset"),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    )
    .await;
    assert!(result.is_ok());
    assert_eq!(
        String::from_utf8(stdout).expect("migration output should be utf-8"),
        "Quota reset moved to codex-router quota: focus an account and press Ctrl-R.\n"
    );
    assert!(stderr.is_empty());
}

#[test]
fn sessions_sql_boundary_uses_sqlx_without_rusqlite() {
    let sessions_source = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../agent-sessions/src/session_commands/session_catalog_query.rs"),
    ));

    assert!(sessions_source.contains("use sqlx::"));
    let catalog_source = must_ok(fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../codex-native-integration/src/stored_thread_catalog.rs"),
    ));
    assert!(sessions_source.contains("StoredThreadCatalog::open"));
    assert!(catalog_source.contains("SqliteConnectOptions"));
    assert!(catalog_source.contains(".read_only(true)"));
    assert!(!catalog_source.contains("rusqlite"));
    assert!(!sessions_source.contains("rusqlite"));

    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| panic!("cli crate should have workspace root parent"));
    let committed_diff = git_diff_text(
        workspace_root,
        &["diff", "--unified=0", "origin/main...HEAD", "--", "crates"],
    );
    let worktree_diff = git_diff_text(workspace_root, &["diff", "--unified=0", "--", "crates"]);
    let diff_text = format!("{committed_diff}\n{worktree_diff}");
    let added_rusqlite_lines = diff_text
        .lines()
        .filter(|line| {
            line.starts_with('+')
                && !line.starts_with("+++")
                && (line.contains(concat!("rus", "qlite::"))
                    || line.contains(concat!("use rus", "qlite"))
                    || line.contains(concat!(" rus", "qlite ")))
        })
        .collect::<Vec<_>>();
    assert!(
        added_rusqlite_lines.is_empty(),
        "new or extended SQL must be SQLx-only; added disallowed binding lines: {added_rusqlite_lines:?}"
    );
}
