use super::*;

#[test]
fn profile_render_includes_codex_custom_provider_contract() {
    let profile = CodexRouterProfile::new(8787);
    let rendered = profile.render();

    assert!(!rendered.contains("[profiles.codex-router]\n"));
    assert!(rendered.contains("model_provider = \"codex-router\"\n"));
    assert!(rendered.contains("[model_providers.codex-router]\n"));
    assert!(rendered.contains("name = \"codex-router\"\n"));
    assert!(rendered.contains("base_url = \"http://127.0.0.1:8787/v1\"\n"));
    assert!(rendered.contains("wire_api = \"responses\"\n"));
    assert!(rendered.contains("requires_openai_auth = false\n"));
    assert!(rendered.contains("supports_websockets = true\n"));
    assert!(!rendered.contains("env_key"));
    assert!(!rendered.contains("env_http_headers"));
    assert!(!rendered.contains("sk-"));
    assert!(!rendered.contains("oauth"));
}

#[test]
fn profile_writer_dry_run_and_approval_gate_do_not_touch_real_codex_home() {
    let test_root = TestRoot::new("profile");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let profile = CodexRouterProfile::new(8787);
    let writer = CodexRouterProfileWriter::new(&codex_home);

    let dry_run = must_ok(writer.dry_run(&profile));

    assert_eq!(
        dry_run.target_path(),
        codex_home.join("codex-router.config.toml")
    );
    assert!(!dry_run.content().contains("[profiles.codex-router]"));
    assert!(!dry_run.target_path().exists());
    assert_eq!(
        writer.write(&profile, false),
        Err(ProfileWriteError::ApprovalRequired)
    );
    assert!(!dry_run.target_path().exists());

    let written_path = must_ok(writer.write(&profile, true));

    assert_eq!(written_path, dry_run.target_path());
    assert_eq!(must_ok(fs::read_to_string(&written_path)), profile.render());
}

#[test]
fn profile_print_command_renders_profile_without_writing() {
    let test_root = TestRoot::new("profile-print");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "print",
            "--port",
            "9876",
            "--codex-home",
            path_to_str(&codex_home),
        ],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    assert!(!output.stdout.contains("[profiles.codex-router]\n"));
    assert!(
        output
            .stdout
            .contains("model_provider = \"codex-router\"\n")
    );
    assert!(output.stdout.contains("name = \"codex-router\"\n"));
    assert!(
        output
            .stdout
            .contains("base_url = \"http://127.0.0.1:9876/v1\"\n")
    );
    assert!(!codex_home.join("codex-router.config.toml").exists());
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_print_emits_router_custom_provider_without_home_mutation() {
    let test_root = TestRoot::new("profile-print-plan-row");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "print",
            "--port",
            "9876",
            "--codex-home",
            path_to_str(&codex_home),
        ],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    assert_router_profile_contract(&output.stdout, 9876);
    assert!(!codex_home.join("codex-router.config.toml").exists());
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_doctor_reports_tokenless_auth_without_token_value() {
    let output = run_cli(
        ["codex-router", "profile", "doctor"],
        CliContext::new(vec![(
            "CODEX_ROUTER_TOKEN".to_owned(),
            "local-secret-token-canary".to_owned(),
        )]),
    );

    assert!(output.stdout.contains("local router token: not required\n"));
    assert!(!output.stdout.contains("local-secret-token-canary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_dry_run_previews_target_without_writing() {
    let test_root = TestRoot::new("profile-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--port",
            "9876",
            "--dry-run",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output.stdout.contains(
            format!(
                "target: {}",
                codex_home.join("codex-router.config.toml").display()
            )
            .as_str()
        )
    );
    assert!(!output.stdout.contains("preview-token: "));
    assert!(output.stdout.contains("current: <missing>\n"));
    assert!(output.stdout.contains("proposed:\n"));
    assert!(
        output
            .stdout
            .contains("base_url = \"http://127.0.0.1:9876/v1\"\n")
    );
    assert!(!codex_home.join("codex-router.config.toml").exists());
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_dry_run_previews_named_profile_without_mutation() {
    let test_root = TestRoot::new("profile-dry-run-plan-row");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--port",
            "9876",
            "--dry-run",
        ],
        CliContext::new(Vec::new()),
    );

    let target_path = codex_home.join("codex-router.config.toml");
    assert!(
        output
            .stdout
            .contains(format!("target: {}", target_path.display()).as_str())
    );
    assert!(!output.stdout.contains("preview-token: "));
    assert!(output.stdout.contains("current: <missing>\n"));
    assert!(output.stdout.contains("proposed:\n"));
    assert_router_profile_contract(&output.stdout, 9876);
    assert!(!target_path.exists());
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_dry_run_previews_existing_file_delta_without_writing() {
    let test_root = TestRoot::new("profile-existing-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    let target_path = codex_home.join("codex-router.config.toml");
    must_ok(fs::write(
        &target_path,
        "model_provider = \"old-router\"\nlegacy = true\n",
    ));

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--port",
            "9876",
            "--dry-run",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output
            .stdout
            .contains(format!("target: {}", target_path.display()).as_str())
    );
    assert!(!output.stdout.contains("preview-token: "));
    assert!(output.stdout.contains("current:\n"));
    assert!(
        output
            .stdout
            .contains("< model_provider = \"old-router\"\n")
    );
    assert!(output.stdout.contains("< legacy = true\n"));
    assert!(output.stdout.contains("proposed:\n"));
    assert!(
        output
            .stdout
            .contains("> base_url = \"http://127.0.0.1:9876/v1\"\n")
    );
    assert_eq!(
        must_ok(fs::read_to_string(&target_path)),
        "model_provider = \"old-router\"\nlegacy = true\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_dry_run_redacts_existing_secret_values() {
    let test_root = TestRoot::new("profile-redacted-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    let target_path = codex_home.join("codex-router.config.toml");
    must_ok(fs::write(
        &target_path,
        "api_key = \"local-secret-canary\"\nmodel_provider = \"old-router\"\n",
    ));

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--dry-run",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(!output.stdout.contains("local-secret-canary"));
    assert!(output.stdout.contains("< api_key = \"<redacted>\"\n"));
    assert!(
        output
            .stdout
            .contains("< model_provider = \"old-router\"\n")
    );
    assert_eq!(
        must_ok(fs::read_to_string(&target_path)),
        "api_key = \"local-secret-canary\"\nmodel_provider = \"old-router\"\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_rejects_unreadable_existing_profile() {
    let test_root = TestRoot::new("profile-invalid-utf8");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    let target_path = codex_home.join("codex-router.config.toml");
    must_ok(fs::write(&target_path, [0xff, 0xfe, 0xfd]));
    let profile = CodexRouterProfile::new(8787);
    let writer = CodexRouterProfileWriter::new(&codex_home);

    let error = match writer.dry_run(&profile) {
        Ok(_) => panic!("invalid UTF-8 profile must not be previewed as missing"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("profile filesystem error"));
    assert_eq!(must_ok(fs::read(&target_path)), vec![0xff, 0xfe, 0xfd]);
}

#[test]
fn profile_write_command_requires_approval_flag() {
    let test_root = TestRoot::new("profile-write-approval");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "profile".into(),
            "write".into(),
            "--codex-home".into(),
            codex_home.as_os_str().to_owned(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("profile write without approval must fail"),
        Err(error) => error,
    };

    assert_eq!(
        error.to_string(),
        "explicit approval is required before writing Codex profile files"
    );
    assert!(!codex_home.join("codex-router.config.toml").exists());
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn profile_write_command_with_approval_writes_only_temp_codex_home() {
    let test_root = TestRoot::new("profile-write-approved");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--approve-codex-home-write",
        ],
        CliContext::new(Vec::new()),
    );

    let target_path = codex_home.join("codex-router.config.toml");
    assert!(
        output
            .stdout
            .contains(format!("wrote: {}", target_path.display()).as_str())
    );
    assert_eq!(
        must_ok(fs::read_to_string(&target_path)),
        CodexRouterProfile::new(8787).render()
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn profile_write_command_rejects_removed_preview_token_option() {
    let test_root = TestRoot::new("profile-write-preview-removed");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_with_io(
        vec![
            "codex-router".into(),
            "profile".into(),
            "write".into(),
            "--codex-home".into(),
            codex_home.as_os_str().to_owned(),
            "--approve-codex-home-write".into(),
            "--preview-token".into(),
            "old-token-like-value".into(),
        ],
        &CliContext::new(Vec::new()),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("removed preview token option must fail"),
        Err(error) => error,
    };

    assert_eq!(error.to_string(), "unknown option: --preview-token");
    assert!(!codex_home.join("codex-router.config.toml").exists());
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}

#[test]
fn profile_write_approved_writes_only_named_temp_profile_file() {
    let test_root = TestRoot::new("profile-write-plan-row");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");

    let output = run_cli(
        [
            "codex-router",
            "profile",
            "write",
            "--codex-home",
            path_to_str(&codex_home),
            "--port",
            "9876",
            "--approve-codex-home-write",
        ],
        CliContext::new(Vec::new()),
    );

    let target_path = codex_home.join("codex-router.config.toml");
    assert!(
        output
            .stdout
            .contains(format!("wrote: {}", target_path.display()).as_str())
    );
    assert_eq!(
        must_ok(fs::read_to_string(&target_path)),
        CodexRouterProfile::new(9876).render()
    );
    assert!(output.stderr.is_empty());
}
