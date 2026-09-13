use super::*;

#[test]
fn sessions_list_table_renders_human_title_and_metadata() {
    const DERIVED_TITLE: &str = "Derived session title";
    const EXPLICIT_NAME: &str = "Fix session table display";
    let test_root = TestRoot::new("sessions-table");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        DERIVED_TITLE,
        &[CodexStateThreadFixture::new(
            "thread-table",
            &project,
            "codex-router",
            "cli",
            "cli",
            "main",
            1000,
        )
        .with_search_fields(DERIVED_TITLE, "derived preview", "derived message")
        .with_name(EXPLICIT_NAME)],
    );

    let output = run_cli(
        ["--any", "--source", "all", "--list", "--format", "table"],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
    );

    assert!(
        output
            .stdout
            .contains("Fix session table display | Derived session title")
    );
    assert!(output.stdout.contains("main"));
    assert!(output.stdout.contains("thread-…"));
    assert!(!output.stdout.contains("┌"));
    assert!(!output.stdout.contains("╞"));
    assert!(!output.stdout.contains("│ session"));
    assert!(output.stderr.is_empty());
}

#[test]
fn sessions_last_dry_run_prints_codex_resume_command_for_latest_match() {
    let test_root = TestRoot::new("sessions-last-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    let invoking_cwd = must_ok(fs::canonicalize(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "LAST_CANARY_SHOULD_NOT_LEAK",
        &[
            CodexStateThreadFixture::new(
                "thread-old",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                1000,
            ),
            CodexStateThreadFixture::new(
                "thread-new",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                2000,
            ),
        ],
    );

    let output = run_cli(
        [
            "--any",
            "--source",
            "interactive",
            "--provider",
            "codex-router",
            "--last",
            "--dry-run",
        ],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
    );

    assert_eq!(
        output.stdout,
        format!(
            "codex --profile codex-router --remote unix://{} --cd {} resume -- thread-new\n",
            test_root
                .path()
                .join(".codex-router/agent-communication/codex-native.sock")
                .display(),
            invoking_cwd.display(),
        )
    );
}

#[test]
fn sessions_local_last_dry_run_reads_codex_state_without_remote_attachment() {
    let test_root = TestRoot::new("sessions-local-last-dry-run");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    let invoking_cwd = must_ok(fs::canonicalize(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "LOCAL_SESSION",
        &[CodexStateThreadFixture::new(
            "thread-local",
            &project,
            "codex-router",
            "cli",
            "cli",
            "main",
            2000,
        )],
    );

    let output = run_cli(
        [
            "--local",
            "--any",
            "--provider",
            "codex-router",
            "--last",
            "--dry-run",
        ],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
    );

    assert_eq!(
        output.stdout,
        format!(
            "codex --profile codex-router --cd {} resume -- thread-local\n",
            invoking_cwd.display(),
        )
    );
}

#[test]
fn sessions_last_rejects_state_session_id_option_injection() {
    let test_root = TestRoot::new("sessions-last-option-injection");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "UNSAFE_SESSION_CANARY_SHOULD_NOT_LEAK",
        &[CodexStateThreadFixture::new(
            "--dangerously-bypass-approvals-and-sandbox",
            &project,
            "codex-router",
            "cli",
            "cli",
            "main",
            1000,
        )],
    );

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let error = match run_session_cli(
        vec![
            "--any".into(),
            "--source".into(),
            "interactive".into(),
            "--provider".into(),
            "codex-router".into(),
            "--last".into(),
            "--dry-run".into(),
        ],
        &CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
        &mut stdout,
        &mut stderr,
    ) {
        Ok(()) => panic!("unsafe session id must not be rendered or launched"),
        Err(error) => error,
    };

    assert_eq!(error, "unsafe Codex session id in state database");
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}
