use super::*;

#[test]
fn token_export_is_single_assignment_without_prose() {
    let assignment =
        export_token_assignment("CODEX_ROUTER_TOKEN", "quote'and\nnewline", Shell::Posix);

    assert!(assignment.starts_with("export CODEX_ROUTER_TOKEN='"));
    assert!(assignment.ends_with("'\n"));
    assert_eq!(assignment.matches("CODEX_ROUTER_TOKEN=").count(), 1);
    assert!(assignment.contains("'\\''"));
}

#[test]
fn token_service_rotates_through_real_secret_store() {
    let test_root = TestRoot::new("service");
    let store = must_ok(FileSecretStore::open(test_root.path()));
    let service = LocalRouterTokenService::new(store);

    let first = must_ok(service.rotate_with_token("first-token"));
    let second = must_ok(service.rotate_with_token("second-token"));

    assert_eq!(first.generation().as_u64(), 1);
    assert_eq!(second.generation().as_u64(), 2);

    let loaded = must_ok(service.load_current());
    assert_eq!(loaded.token().expose_secret(), "second-token");
    assert_eq!(loaded.generation().as_u64(), 2);
}

#[test]
fn token_export_and_profile_doctor_redact_router_token_value() {
    let test_root = TestRoot::new("token-export-profile-doctor");
    let store = must_ok(FileSecretStore::open(test_root.path()));
    let service = LocalRouterTokenService::new(store);
    let record = must_ok(service.rotate_with_token("router-token-canary"));

    let export_output = run_cli(
        [
            "codex-router",
            "token",
            "export",
            "--router-root",
            path_to_str(test_root.path()),
        ],
        CliContext::new(Vec::new()),
    );
    let doctor_output = run_cli(
        ["codex-router", "profile", "doctor"],
        CliContext::new(vec![(
            "CODEX_ROUTER_TOKEN".to_owned(),
            record.token().expose_secret().to_owned(),
        )]),
    );

    assert!(
        export_output
            .stdout
            .starts_with("export CODEX_ROUTER_TOKEN='")
    );
    assert!(
        doctor_output
            .stdout
            .contains("local router token: not required\n")
    );
    assert!(
        !doctor_output
            .stdout
            .contains(record.token().expose_secret())
    );
    assert!(export_output.stderr.is_empty());
    assert!(doctor_output.stderr.is_empty());
}

#[test]
fn token_export_command_emits_current_router_root_token_assignment() {
    let test_root = TestRoot::new("token-export-command");
    let store = must_ok(FileSecretStore::open(test_root.path()));
    let service = LocalRouterTokenService::new(store);
    let record = must_ok(service.rotate_with_token("quote'and\nnewline"));

    let output = run_cli(
        [
            "codex-router",
            "token",
            "export",
            "--router-root",
            path_to_str(test_root.path()),
            "--shell",
            "posix",
        ],
        CliContext::new(Vec::new()),
    );

    assert_eq!(
        output.stdout,
        export_token_assignment(
            "CODEX_ROUTER_TOKEN",
            record.token().expose_secret(),
            Shell::Posix
        )
    );
    assert_eq!(output.stdout.matches("CODEX_ROUTER_TOKEN=").count(), 1);
    assert!(output.stderr.is_empty());
}

#[test]
fn token_init_and_rotate_commands_do_not_print_secret_and_update_export() {
    let test_root = TestRoot::new("token-init-rotate-command");

    let init_output = run_cli(
        [
            "codex-router",
            "token",
            "init",
            "--router-root",
            path_to_str(test_root.path()),
        ],
        CliContext::new(Vec::new()),
    );
    assert_eq!(init_output.stdout, "generation: 1\n");
    assert!(!init_output.stdout.contains("CODEX_ROUTER_TOKEN="));
    assert!(init_output.stderr.is_empty());

    let first_export = run_cli(
        [
            "codex-router",
            "token",
            "export",
            "--router-root",
            path_to_str(test_root.path()),
        ],
        CliContext::new(Vec::new()),
    );
    assert!(
        first_export
            .stdout
            .starts_with("export CODEX_ROUTER_TOKEN='")
    );

    let rotate_output = run_cli(
        [
            "codex-router",
            "token",
            "rotate",
            "--router-root",
            path_to_str(test_root.path()),
        ],
        CliContext::new(Vec::new()),
    );
    assert_eq!(rotate_output.stdout, "generation: 2\n");
    assert!(!rotate_output.stdout.contains("CODEX_ROUTER_TOKEN="));
    assert!(rotate_output.stderr.is_empty());

    let second_export = run_cli(
        [
            "codex-router",
            "token",
            "export",
            "--router-root",
            path_to_str(test_root.path()),
        ],
        CliContext::new(Vec::new()),
    );
    assert!(
        second_export
            .stdout
            .starts_with("export CODEX_ROUTER_TOKEN='")
    );
    assert_ne!(first_export.stdout, second_export.stdout);
}

#[test]
fn token_export_command_defaults_to_home_router_secret_root() {
    let command = match CliCommand::parse([
        OsString::from("token"),
        OsString::from("export"),
        OsString::from("--shell"),
        OsString::from("posix"),
    ]) {
        Ok(CliCommand::Token(command)) => command,
        Ok(other) => panic!("token command should parse, got {other:?}"),
        Err(error) => panic!("token command should parse: {error}"),
    };

    let TokenCommand::Export { router_root, shell } = command else {
        panic!("token export command should parse");
    };
    assert_eq!(router_root, default_router_secret_root_for_test());
    assert_eq!(shell, Shell::Posix);
}
