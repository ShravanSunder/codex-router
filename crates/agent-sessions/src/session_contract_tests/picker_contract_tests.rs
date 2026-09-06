use super::*;

#[test]
fn sessions_last_launch_uses_injected_runner_for_latest_match() {
    let test_root = TestRoot::new("sessions-last-runner");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "RUNNER_CANARY_SHOULD_NOT_LEAK",
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
    let command = match parse_session_arguments([
        OsString::from("--any"),
        OsString::from("--provider"),
        OsString::from("codex-router"),
        OsString::from("--last"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
        ("CODEX_ROUTER_FORCE_NON_TTY".to_owned(), "1".to_owned()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new("unused");
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert_eq!(runner.resumed_session_ids, ["thread-new"]);
}

#[test]
fn sessions_interactive_picker_launches_selected_session_with_injected_dependencies() {
    let test_root = TestRoot::new("sessions-picker-runner");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "PICKER_CANARY_SHOULD_NOT_LEAK",
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
            CodexStateThreadFixture::new(
                "thread-sibling",
                &test_root.path().join("sibling-project"),
                "openai",
                "cli",
                "cli",
                "feature",
                3000,
            ),
            CodexStateThreadFixture::new(
                "thread-subagent",
                &project,
                "codex-router",
                "subagent",
                "subagent",
                "main",
                4000,
            ),
        ],
    );
    let command = match parse_session_arguments([]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new("thread-old");
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert_eq!(picker.offered_session_ids, ["thread-new", "thread-old"]);
    assert_eq!(picker.offered_labels.len(), 2);
    assert_eq!(picker.offered_labels[0], "PICKER_CANARY_SHOULD_NOT_LEAK");
    assert_eq!(runner.resumed_session_ids, ["thread-old"]);
}

#[test]
fn sessions_interactive_picker_forks_selected_session_with_injected_dependencies() {
    let test_root = TestRoot::new("sessions-picker-fork-runner");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "PICKER_FORK_CANARY",
        &[CodexStateThreadFixture::new(
            "thread-fork-source",
            &project,
            "codex-router",
            "cli",
            "cli",
            "main",
            1000,
        )],
    );
    let command = match parse_session_arguments([OsString::from("--yolo")]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new_fork("thread-fork-source");
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert_eq!(runner.forked_session_ids, ["thread-fork-source"]);
    assert_eq!(runner.fork_codex_args, [[OsString::from("--yolo")]]);
    assert!(runner.resumed_session_ids.is_empty());
}

#[test]
fn sessions_interactive_picker_loads_older_repo_matches_beyond_default_limit() {
    let test_root = TestRoot::new("sessions-picker-older-repo-match");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    let unrelated_project = test_root.path().join("unrelated-project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    must_ok(fs::create_dir(&unrelated_project));

    let target_session_id = "thread-target-older-subagent";
    let mut rows = Vec::new();
    for index in 0..101 {
        rows.push(CodexStateThreadFixture::new(
            &format!("thread-unrelated-newer-{index}"),
            &unrelated_project,
            "codex-router",
            "cli",
            "cli",
            "main",
            10_000 + i64::from(index),
        ));
    }
    rows.push(CodexStateThreadFixture::new(
        target_session_id,
        &project,
        "codex-router",
        "subagent",
        "subagent",
        "main",
        1_000,
    ));
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "PICKER_CANARY_SHOULD_NOT_LEAK",
        &rows,
    );
    let command = match parse_session_arguments([
        OsString::from("--repo"),
        OsString::from("--source"),
        OsString::from("subagents"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new(target_session_id);
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert_eq!(picker.offered_session_ids, [target_session_id]);
    assert_eq!(runner.resumed_session_ids, [target_session_id]);
}

#[test]
fn sessions_picker_loader_pages_until_full_persisted_search_field_matches() {
    let test_root = TestRoot::new("sessions-picker-search-paging");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));

    let mut rows = (0..260)
        .map(|index| {
            CodexStateThreadFixture::new(
                &format!("thread-unmatched-{index}"),
                &project,
                "codex-router",
                "cli",
                "cli",
                "main",
                10_000 + i64::from(index),
            )
        })
        .collect::<Vec<_>>();
    let target_session_id = "thread-full-field-match";
    rows.push(
        CodexStateThreadFixture::new(
            target_session_id,
            &project,
            "codex-router",
            "cli",
            "cli",
            "feature/search",
            1_000,
        )
        .with_search_fields(
            &format!("{} deep-marker", "display-prefix ".repeat(12)),
            "preview without the marker",
            "first message without the marker",
        ),
    );
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "UNMATCHED_SEARCH_CANARY",
        &rows,
    );
    let command = must_ok(parse_session_arguments([OsString::from("--repo")]));
    let query = crate::presentation::session_picker::SessionsPickerDataQuery {
        root: crate::presentation::session_picker::SessionsPickerRoot::Any,
        provider: crate::sessions::SessionsProvider::Any,
        source: crate::sessions::SessionsSource::All,
        sort: crate::sessions::SessionsSort::Updated,
        search: "deep-marker".to_owned(),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new(target_session_id).with_loader_query(query);
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert_eq!(
        picker.loaded_session_ids,
        [vec![target_session_id.to_owned()]]
    );
    assert_eq!(runner.resumed_session_ids, [target_session_id]);
}

#[test]
fn sessions_repo_scope_uses_persisted_origin_for_deleted_worktrees() {
    let test_root = TestRoot::new("sessions-repo-origin");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let current_checkout = test_root.path().join("session-picker-fixture");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&current_checkout));
    let init_status = must_ok(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(&current_checkout)
            .args(["init", "--quiet"])
            .status(),
    );
    assert!(init_status.success());
    let remote_status = must_ok(
        ProcessCommand::new("git")
            .arg("-C")
            .arg(&current_checkout)
            .args([
                "remote",
                "add",
                "origin",
                "https://github.com/shravan-agent/session-picker-fixture.git",
            ])
            .status(),
    );
    assert!(remote_status.success());

    let matching_origin_session = "thread-deleted-matching-origin";
    create_codex_state_db_with_thread_rows(
        &codex_home.join("state_5.sqlite"),
        "ORIGIN_SCOPE_CANARY",
        &[
            CodexStateThreadFixture::new(
                matching_origin_session,
                &test_root.path().join("deleted-worktree-no-name-match"),
                "codex-router",
                "cli",
                "cli",
                "feature/deleted",
                3_000,
            )
            .with_git_origin("git@github.com:shravan-agent/session-picker-fixture.git"),
            CodexStateThreadFixture::new(
                "thread-live-path-conflicting-origin",
                &current_checkout,
                "codex-router",
                "cli",
                "cli",
                "main",
                2_000,
            )
            .with_git_origin("https://github.com/other/session-picker-fixture.git"),
            CodexStateThreadFixture::new(
                "thread-historical-basename-without-origin",
                &test_root.path().join("session-picker-fixture.old"),
                "codex-router",
                "cli",
                "cli",
                "old",
                1_000,
            ),
        ],
    );
    let command = must_ok(parse_session_arguments([OsString::from("--repo")]));
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(current_checkout);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new(matching_origin_session);
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert_eq!(
        picker.offered_session_ids,
        [
            matching_origin_session,
            "thread-historical-basename-without-origin",
        ]
    );
    assert_eq!(runner.resumed_session_ids, [matching_origin_session]);
}

#[test]
fn sessions_interactive_empty_filter_can_start_new_router_profile_session() {
    let test_root = TestRoot::new("sessions-picker-start-new");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(&codex_home.join("state_5.sqlite"), "EMPTY", &[]);
    let command = match parse_session_arguments([]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new_start_new();
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert!(picker.offered_session_ids.is_empty());
    assert_eq!(runner.resumed_session_ids, Vec::<String>::new());
    assert_eq!(runner.new_codex_args, [Vec::<OsString>::new()]);
}

#[test]
fn sessions_interactive_picker_shows_start_new_command_with_passthrough_args() {
    let test_root = TestRoot::new("sessions-picker-start-new-command");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    create_codex_state_db_with_thread_rows(&codex_home.join("state_5.sqlite"), "EMPTY", &[]);
    let command = match parse_session_arguments([
        OsString::from("--yolo"),
        OsString::from("--model"),
        OsString::from("gpt-5-codex"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
    ])
    .with_current_dir(project);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new_start_new();
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &context,
        &mut runner,
        &mut picker,
    ));

    assert!(stdout.is_empty());
    assert_eq!(
        picker.new_session_args_display.as_deref(),
        Some("--yolo --model gpt-5-codex")
    );
    assert_eq!(
        runner.new_codex_args,
        [[
            OsString::from("--yolo"),
            OsString::from("--model"),
            OsString::from("gpt-5-codex")
        ]]
    );
}

#[test]
fn sessions_interactive_non_tty_errors_concisely_without_logs() {
    let test_root = TestRoot::new("sessions-non-tty");
    must_ok(fs::create_dir(test_root.path()));
    let codex_home = test_root.path().join("codex-home");
    let project = test_root.path().join("project");
    must_ok(fs::create_dir(&codex_home));
    must_ok(fs::create_dir(&project));
    let context = CliContext::new(vec![
        ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
        ("HOME".to_owned(), test_root.path().display().to_string()),
        ("CODEX_ROUTER_FORCE_NON_TTY".to_owned(), "1".to_owned()),
    ])
    .with_current_dir(project);
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let error = match run_session_cli([], &context, &mut stdout, &mut stderr) {
        Ok(()) => panic!("non-TTY interactive sessions should fail concisely"),
        Err(error) => error,
    };

    assert_eq!(
        error,
        "sessions interactive picker requires a terminal; use --list or --last"
    );
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
}
