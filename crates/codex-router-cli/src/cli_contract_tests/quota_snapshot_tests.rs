use super::*;
use crate::quota::QuotaWindowHeadroom;
use sqlx::Connection;

pub(super) struct FaultingFloorRefreshProvider {
    pub(super) database_path: PathBuf,
    pub(super) failure_stage: &'static str,
}

impl QuotaRefreshProvider for FaultingFloorRefreshProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if request.route_band() == "responses" {
            let trigger = match self.failure_stage {
                "history" => {
                    "CREATE TRIGGER injected_history_failure BEFORE INSERT ON quota_history_observations BEGIN SELECT RAISE(FAIL, 'injected history failure'); END"
                }
                "snapshot" => {
                    "CREATE TRIGGER injected_snapshot_failure BEFORE INSERT ON quota_snapshots BEGIN SELECT RAISE(FAIL, 'injected snapshot failure'); END"
                }
                _ => panic!("unsupported fault stage"),
            };
            let options = sqlx::sqlite::SqliteConnectOptions::new()
                .filename(&self.database_path)
                .create_if_missing(false);
            let mut connection = sqlx::SqliteConnection::connect_with(&options)
                .await
                .expect("fault connection should open");
            sqlx::query(trigger)
                .execute(&mut connection)
                .await
                .expect("fault should install");
            connection
                .close()
                .await
                .expect("fault connection should close");
        }
        Ok(QuotaRefreshProviderResponse {
            windows: vec![
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 18_000,
                    headroom: QuotaWindowHeadroom::Percent(90),
                    reset_unix_seconds: Some(20_000),
                    effective: true,
                },
                QuotaRefreshProviderWindow {
                    limit_window_seconds: 604_800,
                    headroom: QuotaWindowHeadroom::Percent(8),
                    reset_unix_seconds: Some(614_800),
                    effective: false,
                },
            ],
            reset_credits_available: None,
            ..Default::default()
        })
    }
}

#[test]
fn responses_observation_transaction_failure_sends_no_floor_signal_or_windows() {
    for stage in ["history", "snapshot"] {
        let test_root = TestRoot::new(&format!("floor-refresh-{stage}-fault"));
        must_ok(fs::create_dir(test_root.path()));
        let state_path = test_root.path().join("state.sqlite");
        let secret_root = test_root.path().join("secrets");
        let state = must_ok(SqliteStateStore::open(&state_path));
        let account_id = account_id("floor-fault-account");
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "floor-fault",
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let secrets = must_ok(
            codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
        );
        let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "floor-access-canary",
                        Some("floor-refresh-canary".to_owned()),
                    )
                    .with_expires_unix_seconds(2_000)
                    .to_secret_string(),
                ),
            ),
        );
        test_async_runtime().block_on(async {
            let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
            must_ok(
                mutation
                    .set_weekly_quota_floor_by_account_id(
                        &account_id,
                        Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
                    )
                    .await,
            );
            mutation.close().await;
        });
        let resolver =
            RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
        let provider = FaultingFloorRefreshProvider {
            database_path: state_path.clone(),
            failure_stage: stage,
        };
        let observer = RecordingWeeklyFloorObserver::default();
        let mut output = Vec::new();
        let error = must_err(refresh_quota_store_paths_with_floor_observer(
            &mut output,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds: 1_100,
                schedule: crate::quota::QuotaRefreshSchedule::Manual,
                weekly_floor_observer: Some(&observer),
            },
        ));
        assert!(error.to_string().contains("sqlite state store failed"));
        assert!(lock_test_mutex(&observer.account_ids, "weekly floor observer").is_empty());
        let windows = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            1_100,
        ));
        let saved_new_weekly = windows
            .iter()
            .flat_map(|input| input.windows())
            .any(|window| {
                window.limit_window_seconds() == 604_800 && window.observed_unix_seconds() == 1_100
            });
        assert!(!saved_new_weekly);
    }
}

#[test]
fn quota_refresh_writes_selector_windows_for_runtime_selection() {
    let test_root = TestRoot::new("quota-refresh-selector-windows");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_quota_selector");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "selector",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
            headroom: QuotaWindowHeadroom::Percent(37),
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            headroom: QuotaWindowHeadroom::Percent(15),
            reset_unix_seconds: Some(614_800),
            effective: false,
        },
    ]);
    let mut stdout = Vec::new();
    test_async_runtime().block_on(async {
        let mutation = must_ok(
            AsyncWeeklyQuotaFloorMutationStore::open(&router_root.join("state.sqlite")).await,
        );
        must_ok(
            mutation
                .set_weekly_quota_floor_by_account_id(
                    &account_id,
                    Some(must_ok(WeeklyQuotaFloorBasisPoints::new(1_500))),
                )
                .await,
        );
        mutation.close().await;
    });

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
            schedule: crate::quota::QuotaRefreshSchedule::Manual,
            weekly_floor_observer: Some(&floor_observer),
        },
    ));
    assert_eq!(
        *lock_test_mutex(&floor_observer.account_ids, "weekly floor observer"),
        vec![account_id]
    );
    assert_eq!(
        *lock_test_mutex(&floor_observer.intents, "weekly floor intents"),
        vec![WeeklyQuotaFloorIntent::HardStop]
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
fn quota_refresh_signals_floor_after_saved_history_before_next_account() {
    let test_root = TestRoot::new("quota-refresh-prompt-floor-notification");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let floor_account_id = account_id("acct_a_floor");
    let healthy_account_id = account_id("acct_b_healthy");
    let floor_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        floor_account_id.clone(),
        "floor-account",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let healthy_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
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
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    for (account_id, access_token) in [
        (&floor_account_id, "floor-access-token"),
        (&healthy_account_id, "healthy-access-token"),
    ] {
        let bundle_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
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
    test_async_runtime().block_on(async {
        let mutation = must_ok(
            AsyncWeeklyQuotaFloorMutationStore::open(&router_root.join("state.sqlite")).await,
        );
        must_ok(
            mutation
                .set_weekly_quota_floor_by_account_id(
                    &floor_account_id,
                    Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
                )
                .await,
        );
        mutation.close().await;
    });
    let floor_observer = Arc::new(RecordingWeeklyFloorObserver::default());
    let provider = FloorNotificationOrderingQuotaProvider::new(
        floor_account_id.clone(),
        healthy_account_id,
        Arc::clone(&floor_observer),
        router_root.join("state.sqlite"),
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
            schedule: crate::quota::QuotaRefreshSchedule::Manual,
            weekly_floor_observer: Some(floor_observer.as_ref()),
        },
    ));

    assert_eq!(
        *lock_test_mutex(&floor_observer.account_ids, "weekly floor observer"),
        vec![floor_account_id, provider.healthy_account_id.clone()]
    );
    assert_eq!(
        *lock_test_mutex(&floor_observer.intents, "weekly floor intents"),
        vec![
            WeeklyQuotaFloorIntent::HardStop,
            WeeklyQuotaFloorIntent::Clear
        ]
    );
}

#[test]
fn saved_quota_observations_switch_clear_and_floor_disable_intents() {
    let test_root = TestRoot::new("quota-switch-clear-intents");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_switch_intent");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "switch-intent",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let bundle = AccountCredentialBundle::imported_codex_auth(
        "switch-intent-access",
        Some("switch-intent-refresh".to_owned()),
    )
    .with_expires_unix_seconds(2_000);
    must_ok(secrets.write_secret(&key, &must_ok(bundle.to_secret_string())));
    let resolver =
        RouterCredentialResolver::new(&state, &secrets, NoopCredentialRefreshClient, 1_000);
    test_async_runtime().block_on(async {
        let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
        must_ok(
            mutation
                .set_weekly_quota_floor_by_account_id(
                    &account_id,
                    Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
                )
                .await,
        );
        mutation.close().await;
    });
    let observer = RecordingWeeklyFloorObserver::default();
    for (index, remaining) in [8, 9, 8, 8].into_iter().enumerate() {
        if index == 3 {
            test_async_runtime().block_on(async {
                let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
                must_ok(
                    mutation
                        .set_weekly_quota_floor_by_account_id(&account_id, None)
                        .await,
                );
                mutation.close().await;
            });
        }
        let provider = StaticQuotaRefreshProvider::new(vec![
            QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                headroom: QuotaWindowHeadroom::Percent(100),
                reset_unix_seconds: Some(20_000),
                effective: true,
            },
            QuotaRefreshProviderWindow {
                limit_window_seconds: 604_800,
                headroom: QuotaWindowHeadroom::Percent(remaining),
                reset_unix_seconds: Some(604_800),
                effective: false,
            },
        ]);
        let observed_unix_seconds = 1_100 + u64::try_from(index).expect("small index") * 100;
        let mut output = Vec::new();
        must_ok(refresh_quota_store_paths_with_floor_observer(
            &mut output,
            &state_path,
            &secret_root,
            "https://chatgpt.com/backend-api".to_owned(),
            &resolver,
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds,
                schedule: crate::quota::QuotaRefreshSchedule::Manual,
                weekly_floor_observer: Some(&observer),
            },
        ));
        let saved = must_ok(SelectorQuotaRepository::selector_inputs_for_route_band(
            &state,
            "responses",
            observed_unix_seconds,
        ));
        assert!(saved.iter().any(|account| {
            account.account_id() == &account_id
                && account.windows().iter().any(|window| {
                    window.limit_window_seconds() == 604_800
                        && window.remaining_headroom() == remaining
                })
        }));
    }
    assert_eq!(
        *lock_test_mutex(&observer.intents, "weekly floor intents"),
        vec![
            WeeklyQuotaFloorIntent::GracefulSwitch,
            WeeklyQuotaFloorIntent::Clear,
            WeeklyQuotaFloorIntent::GracefulSwitch,
            WeeklyQuotaFloorIntent::Clear,
        ]
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
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "partial",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
        headroom: QuotaWindowHeadroom::Percent(80),
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
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "missing-reset",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            router_root.join("secrets"),
        ),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
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
            headroom: QuotaWindowHeadroom::Percent(80),
            reset_unix_seconds: Some(20_000),
            effective: true,
        },
        QuotaRefreshProviderWindow {
            limit_window_seconds: 604_800,
            headroom: QuotaWindowHeadroom::Percent(90),
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
