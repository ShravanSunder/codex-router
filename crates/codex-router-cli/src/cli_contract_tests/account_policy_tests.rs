use super::*;

#[test]
fn account_list_command_defaults_to_home_router_root() {
    let command = match CliCommand::parse([OsString::from("account"), OsString::from("list")]) {
        Ok(CliCommand::Account(command)) => command,
        Ok(other) => panic!("account command should parse, got {other:?}"),
        Err(error) => panic!("account command should parse: {error}"),
    };

    let AccountCommand::List { router_root } = command else {
        panic!("account list command should parse");
    };
    assert_eq!(router_root, default_router_root_for_test());
}

#[test]
fn account_set_weekly_floor_parses_exact_contract() {
    let router_root = PathBuf::from("/tmp/codex-router-weekly-floor");
    let command = match CliCommand::parse([
        OsString::from("account"),
        OsString::from("set-weekly-floor"),
        OsString::from("--account"),
        OsString::from("primary"),
        OsString::from("--percent"),
        OsString::from("15"),
        OsString::from("--router-root"),
        router_root.clone().into_os_string(),
    ]) {
        Ok(CliCommand::Account(command)) => command,
        Ok(other) => panic!("account command should parse, got {other:?}"),
        Err(error) => panic!("weekly-floor command should parse: {error}"),
    };

    let AccountCommand::SetWeeklyFloor {
        router_root: parsed_root,
        account_label,
        percent,
    } = command
    else {
        panic!("weekly-floor command should parse");
    };
    assert_eq!(parsed_root, router_root);
    assert_eq!(account_label, "primary");
    assert_eq!(percent, 15);

    let help = match CliCommand::parse([
        OsString::from("account"),
        OsString::from("set-weekly-floor"),
        OsString::from("--help"),
    ]) {
        Ok(CliCommand::Account(AccountCommand::Help(help))) => help,
        Ok(other) => panic!("weekly-floor help should parse, got {other:?}"),
        Err(error) => panic!("weekly-floor help should parse: {error}"),
    };
    assert!(help.contains("--percent <0-15>"));
}

#[test]
fn account_set_weekly_floor_rejects_invalid_and_duplicate_options() {
    for invalid_percent in ["-1", "2.5", "16", "65536"] {
        let result = CliCommand::parse([
            OsString::from("account"),
            OsString::from("set-weekly-floor"),
            OsString::from("--account"),
            OsString::from("primary"),
            OsString::from("--percent"),
            OsString::from(invalid_percent),
        ]);
        assert!(result.is_err(), "{invalid_percent} must be rejected");
    }

    for arguments in [
        vec!["account", "set-weekly-floor", "--percent", "5"],
        vec!["account", "set-weekly-floor", "--account", "primary"],
        vec![
            "account",
            "set-weekly-floor",
            "--account",
            "primary",
            "--account",
            "secondary",
            "--percent",
            "5",
        ],
        vec![
            "account",
            "set-weekly-floor",
            "--account",
            "primary",
            "--percent",
            "5",
            "--percent",
            "6",
        ],
    ] {
        let result = CliCommand::parse(arguments.into_iter().map(OsString::from));
        assert!(result.is_err(), "missing or duplicate options must fail");
    }
}

#[test]
fn account_set_weekly_floor_busy_error_is_exact_and_redacted() {
    let rendered = AccountCommandError::WeeklyFloorDatabaseBusy.to_string();
    assert_eq!(
        rendered,
        "failed to update weekly floor: database is busy; retry the command"
    );
    for canary in ["sensitive-label", "acct_internal", "/private/state.sqlite"] {
        assert!(!rendered.contains(canary));
    }
}

#[test]
fn account_set_weekly_floor_updates_disables_and_lists_policy() {
    let test_root = TestRoot::new("account-weekly-floor");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            account_id("acct_primary"),
            "primary".to_owned(),
            AccountStatus::Enabled,
        ),
    ));
    drop(state);

    let enabled = run_cli(
        [
            "account",
            "set-weekly-floor",
            "--account",
            "primary",
            "--percent",
            "5",
            "--router-root",
            path_to_str(&router_root),
        ],
        CliContext::new(Vec::new()),
    );
    assert_eq!(enabled.stdout, "updated weekly floor: primary = 5%\n");
    assert!(enabled.stderr.is_empty());

    let listed = run_cli(
        [
            "account",
            "list",
            "--router-root",
            path_to_str(&router_root),
        ],
        CliContext::new(Vec::new()),
    );
    assert!(listed.stdout.contains("weekly floor"));
    assert!(listed.stdout.contains("5%"));
    assert!(!listed.stdout.contains("acct_primary"));

    let disabled = run_cli(
        [
            "account",
            "set-weekly-floor",
            "--account",
            "primary",
            "--percent",
            "0",
            "--router-root",
            path_to_str(&router_root),
        ],
        CliContext::new(Vec::new()),
    );
    assert_eq!(
        disabled.stdout,
        "updated weekly floor: primary = disabled (0%)\n"
    );
    assert!(disabled.stderr.is_empty());
}

#[test]
fn account_set_weekly_floor_label_failures_are_redacted_and_do_not_mutate() {
    let test_root = TestRoot::new("account-weekly-floor-labels");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    for (account_id_value, label) in [
        ("acct_duplicate_a", "duplicate"),
        ("acct_duplicate_b", "duplicate"),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                account_id(account_id_value),
                label.to_owned(),
                AccountStatus::Enabled,
            ),
        ));
    }
    drop(state);

    for supplied_label in ["missing-sensitive-label", "duplicate"] {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let result = run_with_io(
            [
                "account",
                "set-weekly-floor",
                "--account",
                supplied_label,
                "--percent",
                "5",
                "--router-root",
                path_to_str(&router_root),
            ]
            .into_iter()
            .map(OsString::from),
            &CliContext::new(Vec::new()),
            &mut stdout,
            &mut stderr,
        );
        let error = result.expect_err("missing or ambiguous label must fail");
        let rendered = error.to_string();
        assert!(stdout.is_empty());
        assert!(stderr.is_empty());
        assert!(!rendered.contains(supplied_label));
        assert!(!rendered.contains("acct_duplicate"));
    }

    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open_read_only(
        &router_root.join("state.sqlite"),
    )));
    let policies = must_ok(runtime.block_on(state.list_account_routing_policies()));
    assert!(policies.is_empty(), "failed label lookup must not mutate");
}

#[test]
fn account_list_renders_friendly_table_without_account_ids() {
    let test_root = TestRoot::new("account-list-table");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            account_id("acct_primary"),
            "primary".to_owned(),
            AccountStatus::Enabled,
        ),
    ));

    let output = run_cli(
        [
            "account",
            "list",
            "--router-root",
            path_to_str(&router_root),
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("account"));
    assert!(output.stdout.contains("status"));
    assert!(output.stdout.contains("primary"));
    assert!(output.stdout.contains("enabled"));
    assert!(!output.stdout.contains("acct_primary"));
    assert!(output.stderr.is_empty());
}
