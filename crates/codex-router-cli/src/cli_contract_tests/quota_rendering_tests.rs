use super::*;

#[test]
fn quota_status_reads_sqlite_rows_without_provider_io() {
    let test_root = TestRoot::new("quota-status");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_primary"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_000)
        .with_route_band("responses", 72)
        .with_reset_unix_seconds(2_000)
        .with_stale_penalty(false),
    ));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_primary"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(1_005)
        .with_route_band("models", 44)
        .with_reset_unix_seconds(3_000),
    ));
    drop(state);
    ensure_async_state_schema(&router_root);

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--no-refresh",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("primary"));
    assert!(output.stdout.contains("72%"));
    assert!(output.stdout.contains("needs refresh"));
    assert!(!output.stdout.contains("acct_primary"));
    assert!(output.stdout.contains("responses"));
    assert!(output.stdout.contains("why"));
    assert!(!output.stdout.contains("models"));
    assert!(!output.stdout.contains("44%"));
    assert!(!output.stdout.contains("pp"));
    assert!(!output.stdout.contains("bottleneck"));
    assert!(!output.stdout.contains("0% left"));
    assert!(!output.stdout.contains("access-token"));
    assert!(!output.stdout.contains("refresh-token"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_mixed_provider_view_shows_openai_and_claude_accounts() {
    let test_root = TestRoot::new("quota-status-openai-provider-filter");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let runtime = test_async_runtime();
    let state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    for (provider, id, label) in [
        (
            codex_router_core::provider::Provider::Openai,
            "acct_openai_view",
            "openai-view-account",
        ),
        (
            codex_router_core::provider::Provider::Claude,
            "acct_claude_view",
            "claude-view-account",
        ),
    ] {
        must_ok(
            runtime.block_on(
                state.upsert_account(
                    &AccountRecord::new(provider, account_id(id), label, AccountStatus::Enabled)
                        .with_active_credential_generation(1),
                ),
            ),
        );
    }
    must_ok(runtime.block_on(state.close()));

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--no-refresh",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(output.stdout.contains("openai-view-account"));
    assert!(output.stdout.contains("claude-view-account"));
    assert!(
        output
            .stdout
            .contains("next claude session → claude-view-account"),
        "{}",
        output.stdout
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_snapshot_rows_show_unknown_pace_until_window_metadata_exists() {
    let test_root = TestRoot::new("quota-status-snapshot-unknown-pace");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_snapshot_pace"),
        "snapshot",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_snapshot_pace"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(9_900)
        .with_route_band("responses", 75)
        .with_reset_unix_seconds(20_000)
        .with_stale_penalty(false),
    ));
    drop(state);
    ensure_async_state_schema(&router_root);

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--no-refresh",
            "--now-unix-seconds",
            "10000",
        ],
        CliContext::new(Vec::new()),
    );

    let lines = output.stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[0],
        concat!("codex-router ", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(lines[1], "pooled credentials\tready");
    assert_eq!(
        lines[2],
        "provider\taccount\tstatus\tOAuth\t5h\tweekly\tweekly floor\treset pace\tsample\tupdated\tclients\tresets available\trouting\tnext use"
    );
    assert!(lines[3].contains("burn unavailable"), "{}", lines[3]);
    assert!(lines[3].contains("sample fresh 1m 40s"), "{}", lines[3]);
    assert!(!lines[3].contains("safe pace"), "{}", lines[3]);
    assert_eq!(
        lines[4],
        "responses route\tnext: snapshot\twhy: fallback by quota: needs refresh limiting window: 5h 75% left"
    );
    assert_eq!(lines.len(), 6);
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_shows_two_user_quota_windows_per_account() {
    let test_root = TestRoot::new("quota-status-all-limits");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    let five_hour_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary"),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(25)
    .with_reset_unix_seconds(20_000)
    .with_effective(true)
    .with_observed_unix_seconds(10_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary"),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(80)
    .with_reset_unix_seconds(614_800)
    .with_observed_unix_seconds(10_000);
    must_ok(
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            primary_account.account_id(),
            "responses",
            &[five_hour_window, weekly_window],
            10_000,
            20_000,
        ),
    );
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_primary"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(10_000)
        .with_route_band("responses", 25)
        .with_reset_unix_seconds(20_000)
        .with_reset_credits_available(1)
        .with_stale_penalty(false),
    ));
    ensure_async_state_schema(&router_root);
    drop(state);

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--all-limits",
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    let lines = output.stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[0],
        concat!("codex-router ", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(lines[1], "pooled credentials\tready");
    assert_eq!(
        lines[2],
        "provider\taccount\tstatus\tOAuth\t5h\tweekly\tweekly floor\treset pace\tsample\tupdated\tclients\tresets available\trouting\tnext use"
    );
    assert!(lines[3].contains("###------- 25% left resets in 2h 30m"));
    assert!(lines[3].contains("########-- 80% left resets in 6d 23h"));
    assert!(
        lines[3].contains("reset pace") || lines[3].contains("burn unavailable"),
        "{}",
        lines[3]
    );
    assert!(lines[3].contains("sample stale 16m 40s"));
    assert!(!lines[3].contains("history unknown"));
    assert!(!lines[3].contains("quota guard"));
    assert_eq!(
        lines[4],
        "responses route\tnext: primary\twhy: preferred by quota: idle far-reset allowance limiting window: 5h 25% left"
    );
    assert_eq!(lines.len(), 6);
    assert!(!output.stdout.contains("acct_primary"));
    assert!(!output.stdout.contains("score"));
    assert!(!output.stdout.contains("risk"));
    assert!(!output.stdout.contains("pressure"));
    assert!(!output.stdout.contains("cost"));
    assert!(!output.stdout.contains("routing_weight"));
    assert!(!output.stdout.contains("active_pressure"));
    assert!(!output.stdout.contains("headroom_cost"));
    assert!(!output.stdout.contains("bottleneck"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_renders_mixed_providers_and_fresh_stale_unknown_claude_windows() {
    use codex_router_core::route_profile::WindowKind;
    use codex_router_state::window_observation::WindowObservation;
    use codex_router_state::window_observation::WindowObservationProps;

    const NOW: u64 = 10_000;
    let test_root = TestRoot::new("quota-status-mixed-provider-claude-windows");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    ensure_async_state_schema(&router_root);

    let openai_account_id = account_id("acct_openai_mixed");
    let fresh_claude_id = account_id("acct_claude_fresh");
    let stale_claude_id = account_id("acct_claude_stale");
    let unknown_claude_id = account_id("acct_claude_unknown");
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let openai_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        openai_account_id.clone(),
        "openai-mixed",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &openai_account,
    ));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(openai_account_id, QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(NOW)
            .with_route_band("responses", 60)
            .with_reset_unix_seconds(NOW + 3_600)
            .with_stale_penalty(false),
    ));
    for (id, label) in [
        (&fresh_claude_id, "claude-fresh"),
        (&stale_claude_id, "claude-stale"),
        (&unknown_claude_id, "claude-unknown"),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Claude,
                id.clone(),
                label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
    }
    drop(state);

    let runtime = test_async_runtime();
    let async_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    for (id, kind, remaining, reset, observed_at) in [
        (
            &fresh_claude_id,
            WindowKind::FiveHour,
            7_500,
            NOW + 3_600,
            NOW,
        ),
        (
            &fresh_claude_id,
            WindowKind::Weekly,
            8_000,
            NOW + 604_800,
            NOW,
        ),
        (
            &stale_claude_id,
            WindowKind::FiveHour,
            9_000,
            NOW + 3_600,
            NOW - 1_000,
        ),
    ] {
        let observation = must_ok(WindowObservation::new(
            WindowObservationProps::new(id.clone(), kind, remaining, observed_at)
                .with_reset_unix_seconds(reset),
        ));
        must_ok(runtime.block_on(async_state.record_window_observation(&observation, || NOW)));
    }
    must_ok(runtime.block_on(async_state.close()));

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--all-limits",
            "--no-refresh",
            "--now-unix-seconds",
            "10000",
        ],
        CliContext::new(vec![("CODEX_ROUTER_FORCE_TTY".to_owned(), "1".to_owned())]),
    );

    assert!(output.stdout.contains("openai-mixed"));
    let fresh_line = output
        .stdout
        .lines()
        .find(|line| line.contains("claude-fresh"))
        .expect("fresh Claude account should render");
    assert!(fresh_line.contains("75% left"), "{fresh_line}");
    assert!(fresh_line.contains("80% left"), "{fresh_line}");
    assert!(fresh_line.contains("sample fresh"), "{fresh_line}");
    assert!(fresh_line.contains("resets in"), "{fresh_line}");
    let stale_line = output
        .stdout
        .lines()
        .find(|line| line.contains("claude-stale"))
        .expect("stale Claude account should render");
    assert!(stale_line.contains("sample stale"), "{stale_line}");
    let unknown_line = output
        .stdout
        .lines()
        .find(|line| line.contains("claude-unknown"))
        .expect("unknown Claude account should render");
    assert!(unknown_line.contains("no data") || unknown_line.contains("needs refresh"));
    assert!(output.stdout.contains("next claude session → claude-fresh"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_table_format_renders_account_rows_without_legacy_tables() {
    let test_root = TestRoot::new("quota-status-account-rows");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    let five_hour_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary"),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(25)
    .with_reset_unix_seconds(20_000)
    .with_effective(true)
    .with_observed_unix_seconds(10_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary"),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(80)
    .with_reset_unix_seconds(614_800)
    .with_observed_unix_seconds(10_000);
    must_ok(
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            primary_account.account_id(),
            "responses",
            &[five_hour_window, weekly_window],
            10_000,
            20_000,
        ),
    );
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_primary"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(10_000)
        .with_route_band("responses", 25)
        .with_reset_unix_seconds(20_000)
        .with_reset_credits_available(1)
        .with_stale_penalty(false),
    ));
    ensure_async_state_schema(&router_root);
    drop(state);

    let output = run_static_quota_cli([
        "codex-router",
        "quota",
        "status",
        "--router-root",
        path_to_str(&router_root),
        "--format",
        "table",
        "--all-limits",
        "--no-refresh",
        "--now-unix-seconds",
        "11000",
    ]);

    let visible_stdout = strip_ansi_sequences(&output.stdout);

    assert!(visible_stdout.starts_with("╭"));
    assert!(visible_stdout.contains("Quota status"));
    assert!(visible_stdout.contains("─"));
    assert!(visible_stdout.contains("╰"));
    assert!(visible_stdout.contains("responses -> primary"));
    assert!(visible_stdout.contains("idle far-reset allowance"));
    assert!(visible_stdout.contains("burn "));
    assert!(!visible_stdout.contains("why: preferred by quota"));
    assert!(!visible_stdout.contains("  Account"));
    assert!(visible_stdout.contains("❯ primary"));
    assert!(visible_stdout.contains("preferred"));
    assert!(visible_stdout.contains("25% left, reset 2h 30m"));
    assert!(visible_stdout.contains("80% left, reset 6d 23h"));
    assert!(visible_stdout.contains("Selected account"));
    assert!(visible_stdout.contains("Activity"));
    assert!(visible_stdout.contains("pace"));
    assert!(visible_stdout.contains("rate"));
    assert!(visible_stdout.contains("guards"));
    assert!(!visible_stdout.contains("│ Clients"));
    assert!(!visible_stdout.contains("account ┆ status"));
    assert!(!visible_stdout.contains("route     ┆ next"));
    assert!(!visible_stdout.contains("acct_primary"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_default_keeps_single_human_status_block_without_refresh() {
    let test_root = TestRoot::new("quota-status-default-readonly-single-block");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_primary"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(10_000)
        .with_route_band("responses", 25)
        .with_reset_unix_seconds(20_000)
        .with_stale_penalty(false),
    ));
    ensure_async_state_schema(&router_root);
    drop(state);

    let output = run_static_quota_cli([
        "codex-router",
        "quota",
        "status",
        "--router-root",
        path_to_str(&router_root),
        "--format",
        "table",
        "--now-unix-seconds",
        "11000",
    ]);

    assert_eq!(output.stdout.matches("Quota status").count(), 1);
    assert!(output.stdout.contains("responses -> primary"));
    assert!(!output.stdout.contains("refresh failed:"));
    assert!(!output.stdout.contains("refreshing quota..."));
    assert!(!output.stdout.contains("updated quota:"));
    assert!(output.stdout.contains("┌"));
    assert!(output.stdout.contains("Selected account"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_redacts_unsafe_account_labels() {
    let test_root = TestRoot::new("quota-status-redacts-unsafe-account-labels");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_unsafe_status_label"),
        "person@example.com",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(
            account_id("acct_unsafe_status_label"),
            QuotaSnapshotSource::MockEndpoint,
        )
        .with_observed_unix_seconds(10_000)
        .with_route_band("responses", 80)
        .with_reset_unix_seconds(20_000)
        .with_stale_penalty(false),
    ));
    ensure_async_state_schema(&router_root);
    drop(state);

    let output = run_static_quota_cli([
        "codex-router",
        "quota",
        "status",
        "--router-root",
        path_to_str(&router_root),
        "--format",
        "table",
        "--now-unix-seconds",
        "11000",
    ]);

    assert!(!output.stdout.contains("person@example.com"));
    assert!(output.stdout.contains("acct-"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_command_defaults_to_home_router_root() {
    let command = match CliCommand::parse([
        OsString::from("quota"),
        OsString::from("status"),
        OsString::from("--now-unix-seconds"),
        OsString::from("0"),
    ]) {
        Ok(CliCommand::Quota(command)) => command,
        Ok(other) => panic!("quota command should parse, got {other:?}"),
        Err(error) => panic!("quota command should parse: {error}"),
    };

    let QuotaCommand::Status { router_root, .. } = command else {
        panic!("quota status command should parse");
    };
    assert_eq!(router_root, default_router_root_for_test());
}

#[test]
fn quota_status_no_refresh_is_available_on_bare_and_explicit_status_forms() {
    for arguments in [
        vec![OsString::from("quota"), OsString::from("--no-refresh")],
        vec![
            OsString::from("quota"),
            OsString::from("status"),
            OsString::from("--no-refresh"),
        ],
    ] {
        let command = match CliCommand::parse(arguments) {
            Ok(CliCommand::Quota(command)) => command,
            Ok(other) => panic!("quota command should parse, got {other:?}"),
            Err(error) => panic!("quota command should parse: {error}"),
        };

        let QuotaCommand::Status { .. } = command else {
            panic!("quota --no-refresh should parse as status");
        };
    }
}
