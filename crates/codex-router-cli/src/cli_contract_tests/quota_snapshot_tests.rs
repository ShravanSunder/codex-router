use super::*;

#[test]
fn quota_refresh_writes_selector_windows_for_runtime_selection() {
    let test_root = TestRoot::new("quota-refresh-selector-windows");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_selector");
    let account = AccountRecord::new(account_id.clone(), "selector", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "quota-selector-access-token",
                    Some("quota-selector-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(2_000)
                .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = StaticQuotaRefreshProvider::new(vec![
        QuotaRefreshProviderWindow {
            limit_window_seconds: 18_000,
            remaining_headroom: 37,
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            remaining_headroom: 15,
            reset_unix_seconds: Some(614_800),
            effective: false,
        },
    ]);
    let mut stdout = Vec::new();
    let mutation = must_ok(test_async_runtime().block_on(
        AsyncWeeklyQuotaFloorMutationStore::open(&router_root.join("state.sqlite")),
    ));
    must_ok(
        test_async_runtime().block_on(mutation.set_weekly_quota_floor_by_account_id(
            &account_id,
            Some(must_ok(WeeklyQuotaFloorBasisPoints::new(1_500))),
        )),
    );
    test_async_runtime().block_on(mutation.close());
    let floor_observer = RecordingWeeklyFloorObserver::default();

    must_ok(refresh_quota_store_paths_with_floor_observer(
        &mut stdout,
        &router_root.join("state.sqlite"),
        &router_root.join("secrets"),
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        QuotaRefreshObservationContext {
            observed_unix_seconds: 1_100,
            weekly_floor_observer: Some(&floor_observer),
        },
    ));
    assert_eq!(
        *lock_test_mutex(&floor_observer.account_ids, "weekly floor observer"),
        vec![account_id]
    );

    let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        "responses",
        1_100,
    ));
    assert_eq!(selector_inputs.len(), 1);
    let windows = selector_inputs[0].windows();
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].limit_window_seconds(), 18_000);
    assert_eq!(windows[0].status(), SelectorQuotaWindowStatus::Eligible);
    assert_eq!(windows[0].remaining_headroom(), 37);
    assert_eq!(windows[0].reset_unix_seconds(), Some(20_000));
    assert_eq!(windows[0].observed_unix_seconds(), 1_100);
    assert!(windows[0].effective());
    assert_eq!(windows[1].limit_window_seconds(), 604_800);
    assert_eq!(windows[1].status(), SelectorQuotaWindowStatus::Eligible);
    assert_eq!(windows[1].remaining_headroom(), 15);
    assert_eq!(windows[1].reset_unix_seconds(), Some(614_800));
    assert_eq!(windows[1].observed_unix_seconds(), 1_100);
    assert!(!windows[1].effective());
    assert_eq!(must_ok(String::from_utf8(stdout)), "refreshed: 2\n");

    let status_output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    assert!(status_output.stdout.contains("selector"));
    assert!(status_output.stdout.contains("next"));
    assert!(!status_output.stdout.contains("needs probe"));
    assert!(status_output.stderr.is_empty());
}

#[test]
fn quota_refresh_defers_floor_notification_until_complete_account_batch() {
    let test_root = TestRoot::new("quota-refresh-deferred-floor-notification");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let floor_account_id = account_id("acct_a_floor");
    let healthy_account_id = account_id("acct_b_healthy");
    let floor_account = AccountRecord::new(
        floor_account_id.clone(),
        "floor-account",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let healthy_account = AccountRecord::new(
        healthy_account_id.clone(),
        "healthy-account",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &floor_account,
    ));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &healthy_account,
    ));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    for (account_id, access_token) in [
        (&floor_account_id, "floor-access-token"),
        (&healthy_account_id, "healthy-access-token"),
    ] {
        let bundle_key = must_ok(account_credential_bundle_key(account_id, 1));
        must_ok(
            secrets.write_secret(
                &bundle_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        access_token,
                        Some(format!("{access_token}-refresh")),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
    }
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let mutation = must_ok(test_async_runtime().block_on(
        AsyncWeeklyQuotaFloorMutationStore::open(&router_root.join("state.sqlite")),
    ));
    must_ok(
        test_async_runtime().block_on(mutation.set_weekly_quota_floor_by_account_id(
            &floor_account_id,
            Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
        )),
    );
    test_async_runtime().block_on(mutation.close());
    let floor_observer = Arc::new(RecordingWeeklyFloorObserver::default());
    let provider = FloorNotificationOrderingQuotaProvider::new(
        floor_account_id.clone(),
        healthy_account_id,
        Arc::clone(&floor_observer),
    );
    let mut stdout = Vec::new();

    must_ok(refresh_quota_store_paths_with_floor_observer(
        &mut stdout,
        &router_root.join("state.sqlite"),
        &router_root.join("secrets"),
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        QuotaRefreshObservationContext {
            observed_unix_seconds: 1_100,
            weekly_floor_observer: Some(floor_observer.as_ref()),
        },
    ));

    assert_eq!(
        *lock_test_mutex(&floor_observer.account_ids, "weekly floor observer"),
        vec![floor_account_id]
    );
}

#[test]
fn quota_refresh_weekly_only_response_is_known_with_five_hour_no_data() {
    let test_root = TestRoot::new("quota-refresh-partial-window");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_partial");
    let account = AccountRecord::new(account_id.clone(), "partial", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "partial-quota-token",
                    Some("partial-quota-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(2_000)
                .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = StaticQuotaRefreshProvider::new(vec![QuotaRefreshProviderWindow {
        limit_window_seconds: 604_800,
        remaining_headroom: 80,
        reset_unix_seconds: Some(20_000),
        effective: true,
    }]);
    let mut stdout = Vec::new();

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root.clone(),
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));

    let selector_inputs = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
        &state,
        "responses",
        1_100,
    ));
    assert_eq!(selector_inputs[0].windows().len(), 1);
    let status_output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    assert!(status_output.stdout.contains("partial"));
    assert!(status_output.stdout.contains("no data"));
    assert!(!status_output.stdout.contains("same unknown pool"));
    assert!(
        !status_output
            .stdout
            .contains("fallback by quota: needs refresh")
    );
    assert!(
        status_output
            .stdout
            .contains("responses route\tnext: partial")
    );
    assert!(!status_output.stdout.contains("partial-quota-token"));
    assert!(status_output.stderr.is_empty());

    let json_status_output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    assert!(json_status_output.stdout.contains(r#""no_data""#));
    assert!(
        !json_status_output
            .stdout
            .contains(r#""unknown_fallback_only""#)
    );
    assert!(!json_status_output.stdout.contains("partial-quota-token"));
    assert!(json_status_output.stderr.is_empty());
}

#[test]
fn quota_refresh_missing_reset_response_is_unknown_fallback() {
    let test_root = TestRoot::new("quota-refresh-missing-reset");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_missing_reset");
    let account = AccountRecord::new(account_id.clone(), "missing-reset", AccountStatus::Enabled)
        .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(FileSecretStore::open(router_root.join("secrets")));
    let bundle_key = must_ok(account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "missing-reset-quota-token",
                    Some("missing-reset-quota-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(2_000)
                .to_secret_string(),
            ),
        ),
    );
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    let provider = StaticQuotaRefreshProvider::new(vec![
        QuotaRefreshProviderWindow {
            limit_window_seconds: 18_000,
            remaining_headroom: 80,
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            remaining_headroom: 90,
            reset_unix_seconds: None,
            effective: false,
        },
    ]);
    let mut stdout = Vec::new();

    must_ok(refresh_quota_with_dependencies(
        &mut stdout,
        router_root.clone(),
        "https://chatgpt.com/backend-api".to_owned(),
        &resolver,
        &provider,
        1_100,
    ));

    let status_output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "plain",
            "--now-unix-seconds",
            "1100",
        ],
        CliContext::new(Vec::new()),
    );
    assert!(status_output.stdout.contains("missing-reset"));
    assert!(status_output.stdout.contains("needs refresh"));
    assert!(
        status_output
            .stdout
            .lines()
            .any(|line| line.ends_with("\tfallback by quota"))
    );
    assert!(
        status_output
            .stdout
            .contains("fallback by quota: needs refresh")
    );
    assert!(
        status_output
            .stdout
            .contains("responses route\tnext: missing-reset")
    );
    assert!(!status_output.stdout.contains("missing-reset-quota-token"));
    assert!(status_output.stderr.is_empty());
}
