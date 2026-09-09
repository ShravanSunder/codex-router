use super::*;

#[test]
fn sessions_list_json_reads_codex_state_metadata_without_prompt_leak() {
    const PROMPT_CANARY: &str = "SECRET_PROMPT_CANARY_SHOULD_NOT_LEAK";
    let test_root = TestRoot::new("sessions-list-json");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    create_codex_state_db_with_threads(&codex_home.join("state_5.sqlite"), PROMPT_CANARY);

    let output = run_cli(
        ["--any", "--source", "all", "--list", "--format", "json"],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ]),
    );

    assert!(
        !output.stdout.contains(PROMPT_CANARY),
        "sessions JSON must not leak prompt-derived title, preview, or first message"
    );
    let sessions: serde_json::Value = must_ok(serde_json::from_str(&output.stdout));
    let sessions = match sessions.as_array() {
        Some(sessions) => sessions,
        None => panic!("sessions output should be array"),
    };
    assert_eq!(sessions.len(), 2);
    assert_eq!(sessions[0]["session_id"], "thread-newer");
    assert_eq!(sessions[0]["provider"], "codex-router");
    assert_eq!(sessions[0]["model"], "gpt-5.4-mini");
    assert_eq!(sessions[0]["source"], "cli");
    assert_eq!(sessions[0]["thread_source"], "cli");
    assert_eq!(sessions[0]["git_branch"], "main");
    assert_eq!(
        sessions[0]["cwd"],
        test_root.path().join("project-a").display().to_string()
    );
    assert_eq!(sessions[1]["session_id"], "thread-older");
}

#[test]
fn sessions_codex_state_reader_returns_busy_immediately_and_leaves_state_unchanged() {
    let test_root = TestRoot::new("sessions-state-busy");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    let state_database_path = codex_home.join("state_5.sqlite");
    create_codex_state_db_with_threads(&state_database_path, "busy-canary");
    let state_before = must_ok(fs::read(&state_database_path));

    let runtime = test_async_runtime();
    let owner_pool = runtime.block_on(async {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(&state_database_path)
            .create_if_missing(false);
        must_ok(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options)
                .await,
        )
    });
    runtime.block_on(async {
        must_ok(sqlx::query("BEGIN EXCLUSIVE").execute(&owner_pool).await);
    });

    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ]);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let started_at = Instant::now();
    let error = run_session_cli(
        vec![
            "--any".into(),
            "--source".into(),
            "all".into(),
            "--list".into(),
            "--format".into(),
            "json".into(),
        ],
        &context,
        &mut stdout,
        &mut stderr,
    )
    .expect_err("Codex owner lock must win immediately");
    let elapsed = started_at.elapsed();

    assert!(
        error.to_lowercase().contains("locked") || error.to_lowercase().contains("busy"),
        "expected SQLITE_BUSY/locked, got {error}"
    );
    assert!(
        elapsed < Duration::from_millis(250),
        "read-only guest must not wait or retry while Codex owns the database: {elapsed:?}"
    );
    assert!(stdout.is_empty());

    runtime.block_on(async {
        must_ok(sqlx::query("ROLLBACK").execute(&owner_pool).await);
        owner_pool.close().await;
    });
    assert_eq!(
        must_ok(fs::read(&state_database_path)),
        state_before,
        "session discovery must not mutate Codex state"
    );
}

#[test]
fn sessions_list_json_respects_limit_after_filtering_matches() {
    const PROMPT_CANARY: &str = "LIMIT_CANARY_SHOULD_NOT_LEAK";
    let test_root = TestRoot::new("sessions-limit");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        PROMPT_CANARY,
        &[
            CodexStateThreadFixture::new(
                "thread-third",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                3_000,
            ),
            CodexStateThreadFixture::new(
                "thread-second",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                2_000,
            ),
            CodexStateThreadFixture::new(
                "thread-first",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                1_000,
            ),
        ],
    );

    let output = run_cli(
        [
            "--any",
            "--source",
            "interactive",
            "--list",
            "--format",
            "json",
            "--limit",
            "2",
        ],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
    );

    assert_session_ids(&output.stdout, &["thread-third", "thread-second"]);
    assert!(!output.stdout.contains(PROMPT_CANARY));
    assert!(output.stderr.is_empty());
}

#[test]
fn sessions_list_json_applies_scope_provider_and_source_filters() {
    const PROMPT_CANARY: &str = "FILTER_CANARY_SHOULD_NOT_LEAK";
    let test_root = TestRoot::new("sessions-filters");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project_a = test_root.path().join("project-a");
    let project_a_src = project_a.join("src");
    let project_a_tools = project_a.join("tools");
    let project_b = test_root.path().join("project-b");
    let project_b_src = project_b.join("src");
    for directory in [
        &codex_home,
        &project_a,
        &project_a_src,
        &project_a_tools,
        &project_b,
        &project_b_src,
    ] {
        must_ok(fs::create_dir(directory));
    }
    for repository in [&project_a, &project_b] {
        let init_status = must_ok(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(repository)
                .args(["init", "--quiet"])
                .status(),
        );
        assert!(init_status.success());
    }

    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        PROMPT_CANARY,
        &[
            CodexStateThreadFixture::new(
                "thread-a-src",
                &project_a_src,
                "codex-router",
                "cli",
                "cli",
                "main",
                4000,
            ),
            CodexStateThreadFixture::new(
                "thread-a-tools",
                &project_a_tools,
                "codex-router",
                "vscode",
                "vscode",
                "main",
                3000,
            ),
            CodexStateThreadFixture::new(
                "thread-a-openai",
                &project_a_src,
                "openai",
                "cli",
                "cli",
                "main",
                2000,
            ),
            CodexStateThreadFixture::new(
                "thread-a-subagent",
                &project_a_src,
                "codex-router",
                "subagent",
                "subagent",
                "main",
                1000,
            ),
            CodexStateThreadFixture::new(
                "thread-a-json-subagent",
                &project_a_src,
                "codex-router",
                r#"{"kind":"subagent","parent_thread_id":"thread-a-src"}"#,
                "cli",
                "main",
                1500,
            )
            .with_thread_source(None),
            CodexStateThreadFixture::new(
                "thread-helper-review",
                &project_a_src,
                "codex-router",
                "vscode",
                "guardian_review",
                "main",
                1700,
            ),
            CodexStateThreadFixture::new(
                "thread-helper-memory",
                &project_a_src,
                "codex-router",
                "cli",
                "memory_consolidation",
                "main",
                1600,
            ),
            CodexStateThreadFixture::new(
                "thread-b-src",
                &project_b_src,
                "codex-router",
                "cli",
                "cli",
                "main",
                5000,
            ),
        ],
    );
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
        ("CODEX_ROUTER_FORCE_NON_TTY".to_owned(), "1".to_owned()),
    ])
    .with_current_dir(project_a_src);

    let cwd_output = run_cli(
        [
            "--provider",
            "codex-router",
            "--source",
            "interactive",
            "--list",
            "--format",
            "json",
        ],
        context.clone(),
    );
    assert_session_ids(&cwd_output.stdout, &["thread-a-src"]);

    let worktree_output = run_cli(
        [
            "--checkout",
            "--provider",
            "codex-router",
            "--source",
            "interactive",
            "--list",
            "--format",
            "json",
        ],
        context.clone(),
    );
    assert_session_ids(&worktree_output.stdout, &["thread-a-src", "thread-a-tools"]);

    let subagent_output = run_cli(
        [
            "--any",
            "--provider",
            "codex-router",
            "--source",
            "subagents",
            "--list",
            "--format",
            "json",
        ],
        context,
    );
    assert_session_ids(
        &subagent_output.stdout,
        &[
            "thread-helper-review",
            "thread-helper-memory",
            "thread-a-json-subagent",
            "thread-a-subagent",
        ],
    );
}

#[test]
fn sessions_provider_current_resolves_codex_router_profile_config() {
    const PROMPT_CANARY: &str = "CURRENT_PROVIDER_CANARY_SHOULD_NOT_LEAK";
    let test_root = TestRoot::new("sessions-current-provider");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    must_ok(fs::write(
        codex_home.join("codex-router.config.toml"),
        "model_provider = \"codex-router\"\n",
    ));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        PROMPT_CANARY,
        &[
            CodexStateThreadFixture::new(
                "thread-router",
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                2000,
            ),
            CodexStateThreadFixture::new(
                "thread-openai",
                &project,
                "openai",
                "cli",
                "cli",
                "main",
                1000,
            ),
        ],
    );

    let output = run_cli(
        [
            "--any",
            "--provider",
            "current",
            "--source",
            "interactive",
            "--list",
            "--format",
            "json",
        ],
        CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ])
        .with_current_dir(project),
    );

    assert_session_ids(&output.stdout, &["thread-router"]);
}

#[test]
fn sessions_provider_current_errors_when_codex_profile_is_missing() {
    let test_root = TestRoot::new("sessions-current-provider-missing");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    must_ok(fs::create_dir(&codex_home));
    create_codex_state_db_with_threads(&codex_home.join("state_5.sqlite"), "unused-canary");

    let error = match run_session_cli(
        [
            "--any",
            "--provider",
            "current",
            "--list",
            "--format",
            "json",
        ]
        .into_iter()
        .map(Into::into),
        &CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), test_root.path().display().to_string()),
        ]),
        &mut Vec::new(),
        &mut Vec::new(),
    ) {
        Ok(()) => panic!("provider=current should fail without Codex provider config"),
        Err(error) => error,
    };

    assert!(error.contains("model_provider"));
    assert!(error.contains("codex-router.config.toml"));
}
