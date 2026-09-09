use super::*;

#[test]
fn sessions_picker_defaults_to_cwd_scope_like_noninteractive_commands() {
    let command = match parse_session_arguments([]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };

    assert_eq!(command.root, crate::sessions::SessionsRoot::Cwd);
    assert_eq!(command.provider, crate::sessions::SessionsProvider::Any);
    assert_eq!(command.source, crate::sessions::SessionsSource::Interactive);
    assert_eq!(command.sort, crate::sessions::SessionsSort::Updated);
    assert!(!command.list);
    assert_eq!(command.format, crate::sessions::SessionsFormat::Table);
    assert!(!command.last);
    assert!(command.id.is_none());
    assert!(!command.new);
    assert!(!command.local);
    assert_eq!(command.limit, 100);
    assert!(!command.dry_run);
    assert!(command.codex_args.is_empty());
}

#[test]
fn noninteractive_list_keeps_cwd_scope_and_explicit_source_filter() {
    let command = parse_session_arguments([OsString::from("--list")]).unwrap();
    assert_eq!(command.root, crate::sessions::SessionsRoot::Cwd);
    assert_eq!(command.source, crate::sessions::SessionsSource::Interactive);
}

#[test]
fn sessions_command_parses_exact_resume_id_with_passthrough_args() {
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";
    let command = match parse_session_arguments([
        OsString::from("--id"),
        OsString::from(SESSION_ID),
        OsString::from("--local"),
        OsString::from("--dry-run"),
        OsString::from("--yolo"),
        OsString::from("--model"),
        OsString::from("gpt-5.6-luna"),
        OsString::from("--request-id"),
        OsString::from("11111111-1111-4111-8111-111111111111"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };

    assert_eq!(command.id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        command.codex_args,
        [
            OsString::from("--yolo"),
            OsString::from("--model"),
            OsString::from("gpt-5.6-luna"),
            OsString::from("--request-id"),
            OsString::from("11111111-1111-4111-8111-111111111111")
        ]
    );
}

#[test]
fn sessions_command_parses_positional_resume_id_as_exact_session() {
    const SESSION_ID: &str = "019ff05e-77c0-7831-8f68-40bf182509f6";
    let command = match parse_session_arguments([
        OsString::from(SESSION_ID),
        OsString::from("--"),
        OsString::from("--model"),
        OsString::from("gpt-5.6-luna"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };

    assert_eq!(command.id.as_deref(), Some(SESSION_ID));
    assert_eq!(
        command.codex_args,
        [OsString::from("--model"), OsString::from("gpt-5.6-luna")]
    );
}

#[test]
fn sessions_command_positional_id_keeps_exact_id_mode_conflicts() {
    const SESSION_ID: &str = "019ff05e-77c0-7831-8f68-40bf182509f6";
    for conflicting_option in ["--new", "--last", "--list"] {
        let error = must_err(parse_session_arguments([
            OsString::from(SESSION_ID),
            OsString::from(conflicting_option),
        ]));
        assert!(
            error.contains("cannot be used with"),
            "unexpected positional id conflict error for {conflicting_option}: {error}"
        );
    }
}

#[test]
fn sessions_command_rejects_non_uuid_resume_ids() {
    for session_id in [
        "thread-name",
        "00000000-0000-4000-8000-00000000000",
        "00000000-0000-4000-8000-000000000001-suffix",
        " 00000000-0000-4000-8000-000000000001",
        "00000000-0000-4000-8000-000000000001 ",
    ] {
        let error = must_err(parse_session_arguments([
            OsString::from("--id"),
            OsString::from(session_id),
        ]));
        assert!(
            error.contains("complete UUID"),
            "unexpected resume id error for {session_id:?}: {error}"
        );
    }
}

#[test]
fn sessions_command_id_conflicts_with_other_selection_modes() {
    for conflicting_option in ["--new", "--last", "--list"] {
        let error = must_err(parse_session_arguments([
            OsString::from("--id"),
            OsString::from("00000000-0000-4000-8000-000000000001"),
            OsString::from(conflicting_option),
        ]));
        assert!(
            error.contains("cannot be used with"),
            "unexpected --id conflict error for {conflicting_option}: {error}"
        );
    }
}

#[test]
fn sessions_exact_resume_id_uses_runner_without_picker_or_state_lookup() {
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";
    let command = match parse_session_arguments([
        OsString::from("--id"),
        OsString::from(SESSION_ID),
        OsString::from("--local"),
        OsString::from("--yolo"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let context = CliContext::new(vec![
        (
            "CODEX_HOME".to_owned(),
            "/path/that/does/not/exist".to_owned(),
        ),
        ("CODEX_ROUTER_FORCE_NON_TTY".to_owned(), "1".to_owned()),
    ]);
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new("picker-must-not-run");
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
    assert_eq!(runner.resumed_session_ids, [SESSION_ID]);
    assert_eq!(runner.resume_codex_args, [[OsString::from("--yolo")]]);
}

#[test]
fn sessions_exact_resume_id_dry_run_uses_invoking_cwd() {
    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";
    let codex_home = PathBuf::from("/tmp/codex-router-sessions-id-home");
    let invoking_cwd = PathBuf::from("/tmp/codex-router-sessions-id-project");
    let command = match parse_session_arguments([
        OsString::from("--id"),
        OsString::from(SESSION_ID),
        OsString::from("--dry-run"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new("picker-must-not-run");
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), codex_home.display().to_string()),
        ])
        .with_current_dir(invoking_cwd.clone()),
        &mut runner,
        &mut picker,
    ));

    assert_eq!(
        String::from_utf8(stdout).unwrap_or_else(|error| panic!("stdout utf8: {error}")),
        format!(
            "codex --profile codex-router --remote unix://{} --cd {} resume -- {SESSION_ID}\n",
            codex_home
                .join(".codex-router/agent-communication/codex-native.sock")
                .display(),
            invoking_cwd.display(),
        )
    );
    assert!(picker.offered_session_ids.is_empty());
}

#[test]
fn sessions_local_new_dry_run_keeps_router_profile_without_remote_attachment() {
    let invoking_cwd = PathBuf::from("/tmp/codex-router-sessions-local-project");
    let command = match parse_session_arguments([
        OsString::from("--local"),
        OsString::from("--new"),
        OsString::from("--dry-run"),
        OsString::from("--model"),
        OsString::from("gpt-5.6-luna"),
        OsString::from("--yolo"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new_start_new();
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &CliContext::new(vec![]).with_current_dir(invoking_cwd.clone()),
        &mut runner,
        &mut picker,
    ));

    assert_eq!(
        String::from_utf8(stdout).unwrap_or_else(|error| panic!("stdout utf8: {error}")),
        format!(
            "codex --profile codex-router --cd {} --model gpt-5.6-luna --yolo\n",
            invoking_cwd.display(),
        )
    );
}

#[test]
fn sessions_command_parses_explicit_filters_for_list_json_last() {
    let command = match parse_session_arguments([
        OsString::from("--any"),
        OsString::from("--provider"),
        OsString::from("current"),
        OsString::from("--source"),
        OsString::from("subagents"),
        OsString::from("--sort"),
        OsString::from("created"),
        OsString::from("--list"),
        OsString::from("--format"),
        OsString::from("json"),
        OsString::from("--limit"),
        OsString::from("25"),
        OsString::from("--last"),
        OsString::from("--yolo"),
        OsString::from("--model"),
        OsString::from("gpt-5.4-mini"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };

    assert_eq!(command.root, crate::sessions::SessionsRoot::Any);
    assert_eq!(command.provider, crate::sessions::SessionsProvider::Current);
    assert_eq!(command.source, crate::sessions::SessionsSource::Subagents);
    assert_eq!(command.sort, crate::sessions::SessionsSort::Created);
    assert!(command.list);
    assert_eq!(command.format, crate::sessions::SessionsFormat::Json);
    assert!(command.last);
    assert!(!command.new);
    assert_eq!(command.limit, 25);
    assert!(!command.dry_run);
    assert_eq!(
        command.codex_args,
        [
            OsString::from("--yolo"),
            OsString::from("--model"),
            OsString::from("gpt-5.4-mini")
        ]
    );
}

#[test]
fn sessions_new_dry_run_prints_codex_command_with_passthrough_flags() {
    let codex_home = PathBuf::from("/tmp/codex-router-sessions-new-home");
    let invoking_cwd = PathBuf::from("/tmp/codex-router-sessions-new-project");
    let command = match parse_session_arguments([
        OsString::from("--new"),
        OsString::from("--dry-run"),
        OsString::from("--yolo"),
        OsString::from("--model"),
        OsString::from("gpt-5.4-mini"),
    ]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };
    let mut runner = FakeSessionsCommandRunner::default();
    let mut picker = FakeSessionsPicker::new_start_new();
    let mut stdout = Vec::new();

    must_ok(crate::sessions::run_sessions_command_with_dependencies(
        &mut stdout,
        command,
        &CliContext::new(vec![
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("HOME".to_owned(), codex_home.display().to_string()),
        ])
        .with_current_dir(invoking_cwd.clone()),
        &mut runner,
        &mut picker,
    ));

    assert_eq!(
        String::from_utf8(stdout).unwrap_or_else(|error| panic!("stdout utf8: {error}")),
        format!(
            "codex --profile codex-router --remote unix://{} --cd {} --yolo --model gpt-5.4-mini\n",
            codex_home
                .join(".codex-router/agent-communication/codex-native.sock")
                .display(),
            invoking_cwd.display(),
        )
    );
    assert!(runner.new_codex_args.is_empty());
    assert!(runner.resumed_session_ids.is_empty());
}

#[test]
fn sessions_command_parses_checkout_and_repo_roots() {
    let checkout_command =
        match parse_session_arguments([OsString::from("--checkout"), OsString::from("--list")]) {
            Ok(command) => command,
            Err(error) => panic!("sessions command should parse: {error}"),
        };
    let repo_command = match parse_session_arguments([OsString::from("--repo")]) {
        Ok(command) => command,
        Err(error) => panic!("sessions command should parse: {error}"),
    };

    assert_eq!(
        checkout_command.root,
        crate::sessions::SessionsRoot::Checkout
    );
    assert_eq!(repo_command.root, crate::sessions::SessionsRoot::Repo);

    let checkout_error = parse_session_arguments([OsString::from("--checkout")])
        .expect_err("interactive checkout should be rejected");
    assert!(checkout_error.contains("--checkout requires --list"));
}

#[test]
fn sessions_command_rejects_legacy_scope_option() {
    let error = must_err(parse_session_arguments([
        OsString::from("--scope"),
        OsString::from("any"),
    ]));

    assert!(
        error.contains("unexpected argument")
            || error.contains("unrecognized option")
            || error.contains("--scope was removed; use --checkout, --repo, or --any"),
        "unexpected legacy sessions scope error: {error}"
    );
}

#[test]
fn sessions_command_rejects_limit_without_list() {
    let interactive_error = must_err(parse_session_arguments([
        OsString::from("--limit"),
        OsString::from("5"),
    ]));
    let last_error = must_err(parse_session_arguments([
        OsString::from("--last"),
        OsString::from("--limit"),
        OsString::from("5"),
    ]));

    assert!(interactive_error.contains("--limit only applies with --list"));
    assert!(last_error.contains("--limit only applies with --list"));
}
