use super::*;

#[test]
fn quota_status_projects_held_switch_and_distinct_saved_floor_thresholds() {
    let test_root = TestRoot::new("quota-status-floor-switch");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let protected_id = account_id("acct_status_floor_source");
    let peer_id = account_id("acct_status_floor_peer");
    for (account_id, label, remaining, weekly_reset) in [
        (&protected_id, "source", 8, 10_000),
        (&peer_id, "peer", 80, 400_000),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let windows = [
            PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                18_000,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(100)
            .with_reset_unix_seconds(18_000)
            .with_effective(true)
            .with_observed_unix_seconds(1_000),
            PersistedSelectorQuotaWindow::new(
                account_id.clone(),
                "responses",
                604_800,
                SelectorQuotaWindowStatus::Eligible,
            )
            .with_remaining_headroom(remaining)
            .with_reset_unix_seconds(weekly_reset)
            .with_observed_unix_seconds(1_000),
        ];
        must_ok(
            SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
                &state,
                account_id,
                "responses",
                &windows,
                1_000,
                2_000,
            ),
        );
    }
    drop(state);
    migrate_test_state_database(&state_path);
    let mutation = must_ok(
        test_async_runtime().block_on(AsyncWeeklyQuotaFloorMutationStore::open(&state_path)),
    );
    must_ok(
        test_async_runtime().block_on(mutation.set_weekly_quota_floor_by_account_id(
            &protected_id,
            Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
        )),
    );
    test_async_runtime().block_on(mutation.close());
    let status_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_cli(
            [
                "codex-router",
                "quota",
                "status",
                "--router-root",
                path_to_str(&router_root),
                "--format",
                "json",
                "--no-refresh",
                "--now-unix-seconds",
                "1100",
            ],
            CliContext::new(Vec::new()),
        )
    }));
    let status = match status_result {
        Ok(status) => status,
        Err(panic_payload) => {
            eprintln!(
                "quota_status_sqlite_failure test_pid={} database_path={}\n{}",
                std::process::id(),
                state_path.display(),
                sqlite_file_open_process_observation(&state_path),
            );
            std::panic::resume_unwind(panic_payload);
        }
    };
    let json: serde_json::Value = must_ok(serde_json::from_str(&status.stdout));
    let accounts = json["accounts"]
        .as_array()
        .expect("accounts should be present");
    let source = accounts
        .iter()
        .find(|account| account["safe_account_label"] == "source")
        .expect("source should be present");
    let peer = accounts
        .iter()
        .find(|account| account["safe_account_label"] == "peer")
        .expect("peer should be present");
    assert_eq!(source["routing_reason"], "held_floor_switch");
    assert_eq!(source["routing_exclusion"], "none");
    assert_eq!(source["preferred_next"], false);
    assert_eq!(peer["preferred_next"], true);
    assert_eq!(source["weekly_quota_floor_percent"], 5);
    assert_eq!(source["weekly_quota_switch_at_percent"], 8);
    assert!(source.get("weekly_quota_effective_stop_percent").is_none());
}

#[test]
fn quota_status_json_exposes_burndown_debug_fields_without_secret_material() {
    let test_root = TestRoot::new("quota-status-json");
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
    let runtime = test_async_runtime();
    let primary_account_id = primary_account.account_id().clone();
    let native_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    let credit_attempt = must_ok(
        runtime.block_on(native_state.begin_credit_refresh_attempt(&primary_account_id, 1)),
    );
    let credit_windows = [
        PersistedSelectorQuotaWindow::new(
            primary_account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(25)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(11_000),
        PersistedSelectorQuotaWindow::new(
            primary_account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(80)
        .with_reset_unix_seconds(614_800)
        .with_observed_unix_seconds(11_000),
    ];
    let credit_history = [
        PersistedQuotaHistoryObservation::new(
            primary_account_id.clone(),
            "primary",
            "responses",
            18_000,
            11_000,
            25,
        )
        .with_reset_unix_seconds(20_000)
        .with_effective(true),
        PersistedQuotaHistoryObservation::new(
            primary_account_id.clone(),
            "primary",
            "responses",
            604_800,
            11_000,
            80,
        )
        .with_reset_unix_seconds(614_800),
    ];
    let credit_snapshot = PersistedQuotaSnapshot::new(
        primary_account_id.clone(),
        QuotaSnapshotSource::OpenAiEndpoint,
    )
    .with_observed_unix_seconds(11_000)
    .with_route_band("responses", 25)
    .with_reset_unix_seconds(20_000)
    .with_reset_credits_available(1)
    .with_stale_penalty(false);
    let provider_credit_observation =
        codex_router_core::credit_usage::CreditProviderObservation::new(
            codex_router_core::credit_usage::CreditAvailability::Available { balance: None },
            codex_router_core::credit_usage::CreditSpendControl::Clear,
            Some(codex_router_core::credit_usage::CreditProviderLimitReason::RateLimitReached),
        );
    assert!(must_ok(runtime.block_on(
        native_state.record_responses_refresh_success(
            codex_router_state::credit_store::ResponsesRefreshSuccessCommit {
                attempt: &credit_attempt,
                selector_windows: &credit_windows,
                observed_unix_seconds: 11_000,
                stale_after_unix_seconds: 20_000,
                provider_observation: &provider_credit_observation,
                history_observations: &credit_history,
                snapshot: &credit_snapshot,
            },
        )
    )));
    assert!(must_ok(runtime.block_on(
        native_state.claim_credential_refresh(&primary_account_id, 1, 2,)
    )));
    must_ok(runtime.block_on(native_state.close()));
    let policy_mutation = must_ok(runtime.block_on(
        codex_router_state::credit_store::AsyncCreditUsagePolicyMutationStore::open(
            &router_root.join("state.sqlite"),
        ),
    ));
    must_ok(
        runtime.block_on(policy_mutation.save_account_credit_usage_policy(
            &primary_account_id,
            Some(1),
            codex_router_core::credit_usage::CreditUsagePolicy::Allow,
        )),
    );
    runtime.block_on(policy_mutation.close());
    let in_progress = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );
    let in_progress_json: serde_json::Value = must_ok(serde_json::from_str(&in_progress.stdout));
    assert_eq!(
        in_progress_json["accounts"][0]["oauth_maintenance_state"],
        "unknown"
    );
    assert!(!in_progress.stdout.contains("reauth_required"));
    let account_list = run_cli(
        [
            "codex-router",
            "account",
            "list",
            "--router-root",
            path_to_str(&router_root),
        ],
        CliContext::new(Vec::new()),
    );
    assert!(account_list.stdout.contains("unknown"));
    assert!(!account_list.stdout.contains("re-login required"));
    let maintenance_store = must_ok(runtime.block_on(AsyncSqliteStateStore::open(
        &router_root.join("state.sqlite"),
    )));
    assert!(must_ok(runtime.block_on(
        maintenance_store.finish_credential_refresh_claim(
            primary_account.account_id(),
            1,
            2,
            codex_router_state::credential_maintenance::CredentialMaintenanceState::Retrying,
            codex_router_state::credential_maintenance::CredentialFailureClass::RateLimited,
            Some(11_500),
        )
    )));
    must_ok(runtime.block_on(maintenance_store.close()));

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );

    let parsed: serde_json::Value = must_ok(serde_json::from_str(&output.stdout));
    assert_eq!(parsed["route_result"], "ok");
    assert_eq!(parsed["app_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(parsed["route_band"], "responses");
    assert_eq!(parsed["selected_pool"], "usable");
    assert_eq!(parsed["selected_pool_reason"], "usable_available");
    assert_eq!(
        parsed["preferred_next_account_hash"].as_str().map(str::len),
        Some(16)
    );
    assert!(parsed.get("weighted_candidates").is_none());
    assert_eq!(
        parsed["accounts"][0]["account_hash"].as_str().map(str::len),
        Some(16)
    );
    assert_eq!(parsed["accounts"][0]["safe_account_label"], "primary");
    assert_eq!(parsed["accounts"][0]["availability"], "usable");
    assert_eq!(parsed["accounts"][0]["freshness"], "fresh");
    assert_eq!(
        parsed["accounts"][0]["routing_reason"],
        "preferred_idle_far_reset"
    );
    assert!(parsed["accounts"][0].get("routing_weight").is_none());
    assert_eq!(parsed["accounts"][0]["preferred_next"], true);
    let credit_usage = &parsed["accounts"][0]["credit_usage"];
    assert_eq!(credit_usage["policy"], "allow");
    assert_eq!(credit_usage["availability"], "available");
    assert!(credit_usage["balance"].is_null());
    assert_eq!(credit_usage["spend_control"], "clear");
    assert_eq!(credit_usage["provider_limit_reason"], "rate_limit_reached");
    assert_eq!(credit_usage["freshness"], "fresh");
    assert_ne!(credit_usage["observation_age"], "unknown");
    assert!(!output.stdout.contains("acct_primary"));
    assert_eq!(parsed["accounts"][0]["reset_credits_available"], 1);
    assert_eq!(parsed["accounts"][0]["active_clients"], 0);
    assert_eq!(
        parsed["accounts"][0]["active_clients_source"],
        "sqlx_mirror"
    );
    assert_eq!(parsed["accounts"][0]["next_use"], "preferred by quota");
    assert!(parsed["accounts"][0]["weekly_quota_switch_at_basis_points"].is_null());
    assert!(parsed["accounts"][0]["weekly_quota_switch_at_percent"].is_null());
    assert!(
        parsed["accounts"][0]
            .get("weekly_quota_effective_stop_basis_points")
            .is_none()
    );
    assert_eq!(parsed["accounts"][0]["oauth_maintenance_state"], "retrying");
    assert_eq!(
        parsed["accounts"][0]["oauth_next_attempt_unix_seconds"],
        11_500
    );
    assert_eq!(parsed["accounts"][0]["oauth_failure_class"], "rate_limited");
    assert!(parsed["accounts"][0].get("short_quota_risk").is_none());
    assert!(parsed["accounts"][0].get("weekly_quota_risk").is_none());
    assert_eq!(parsed["accounts"][0]["short_quota_guard"], 25);
    assert_eq!(parsed["accounts"][0]["weekly_quota_guard"], 20);
    assert_eq!(parsed["accounts"][0]["limiting_window"], "5h");
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["5h"]["evidence_state"],
        "known"
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["5h"]["remaining_headroom"],
        25
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["5h"]["run_rate"]["confidence"],
        "insufficient"
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["weekly"]["remaining_headroom"],
        80
    );
    assert_eq!(
        parsed["accounts"][0]["windows"][0]["remaining_headroom"],
        25
    );
    assert_eq!(
        parsed["accounts"][0]["windows"][1]["remaining_headroom"],
        80
    );
    assert_eq!(
        parsed["accounts"][0]["windows"][1]["run_rate"]["confidence"],
        "insufficient"
    );
    assert!(!output.stdout.contains("access-token"));
    assert!(!output.stdout.contains("refresh-token"));
    assert!(!output.stdout.contains("authorization"));
    assert!(!output.stdout.contains("bottleneck"));
    assert!(!output.stdout.contains("risk"));
    assert!(!output.stdout.contains("pressure"));
    assert!(!output.stdout.contains("cost"));
    assert!(!output.stdout.contains("routing_weight"));
    assert!(!output.stdout.contains("active_pressure"));
    assert!(!output.stdout.contains("headroom_cost"));
    assert!(output.stderr.is_empty());

    let auto_output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );
    let auto_parsed: serde_json::Value = must_ok(serde_json::from_str(&auto_output.stdout));
    assert_eq!(auto_parsed["route_result"], "ok");
    assert_eq!(auto_parsed["route_band"], "responses");
    assert_eq!(auto_parsed["selected_pool"], "usable");
    assert!(auto_parsed.get("cached").is_none());
    assert!(auto_parsed.get("refresh").is_none());
    assert!(auto_parsed.get("updated").is_none());
    assert!(!auto_output.stdout.contains("acct_primary"));
}

#[test]
fn quota_status_selection_uses_active_session_count() {
    let test_root = TestRoot::new("quota-status-active-session-count");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let busy_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_busy"),
        "busy",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    let idle_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_idle"),
        "idle",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &busy_account,
    ));
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &idle_account,
    ));
    for account in [&busy_account, &idle_account] {
        let five_hour_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Eligible,
        )
        .with_remaining_headroom(80)
        .with_reset_unix_seconds(20_000)
        .with_effective(true)
        .with_observed_unix_seconds(10_000);
        let weekly_window = PersistedSelectorQuotaWindow::new(
            account.account_id().clone(),
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
                account.account_id(),
                "responses",
                &[five_hour_window, weekly_window],
                10_000,
                20_000,
            ),
        );
    }
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let async_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(runtime.block_on(async_state.record_active_client_acquired(
        "responses",
        "test-process",
        &ReservationId::new("reservation_busy"),
        busy_account.account_id(),
        10_900,
        8,
    )));
    must_ok(runtime.block_on(async_state.close()));

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );

    let parsed: serde_json::Value = must_ok(serde_json::from_str(&output.stdout));
    assert_eq!(
        parsed["preferred_next_account_hash"].as_str().map(str::len),
        Some(16)
    );
    assert_eq!(parsed["accounts"][0]["safe_account_label"], "busy");
    assert_eq!(parsed["accounts"][0]["active_clients"], 1);
    assert_eq!(
        parsed["accounts"][0]["active_clients_source"],
        "sqlx_mirror"
    );
    assert_eq!(parsed["accounts"][0]["preferred_next"], false);
    assert_eq!(parsed["accounts"][1]["safe_account_label"], "idle");
    assert_eq!(parsed["accounts"][1]["active_clients"], 0);
    assert_eq!(
        parsed["accounts"][1]["active_clients_source"],
        "sqlx_mirror"
    );
    assert_eq!(parsed["accounts"][1]["preferred_next"], true);
    assert!(!output.stdout.contains("acct_busy"));
    assert!(!output.stdout.contains("acct_idle"));
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_marks_active_client_mirror_unavailable_when_rows_are_corrupt() {
    let test_root = TestRoot::new("quota-status-active-unavailable");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let five_hour_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(100)
    .with_reset_unix_seconds(25_400)
    .with_effective(true)
    .with_observed_unix_seconds(10_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account.account_id().clone(),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(20)
    .with_reset_unix_seconds(97_400)
    .with_observed_unix_seconds(10_000);
    must_ok(
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            account.account_id(),
            "responses",
            &[five_hour_window, weekly_window],
            10_000,
            20_000,
        ),
    );
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let async_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    must_ok(runtime.block_on(async_state.close()));
    let mutation = must_ok(runtime.block_on(AsyncWeeklyQuotaFloorMutationStore::open(&state_path)));
    must_ok(runtime.block_on(mutation.set_weekly_quota_floor_by_label(
        "primary",
        Some(must_ok(WeeklyQuotaFloorBasisPoints::new(500))),
    )));
    runtime.block_on(mutation.close());
    runtime.block_on(async {
        let pool = must_ok(
            sqlx::sqlite::SqlitePoolOptions::new()
                .max_connections(1)
                .connect(&format!("sqlite://{}", state_path.display()))
                .await,
        );
        must_ok(
            sqlx::query(
                "INSERT INTO active_client_leases (
                       route_band, process_run_id, reservation_id, account_id,
                       acquired_unix_seconds, active_pressure
                     )
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind("responses")
            .bind("process-corrupt")
            .bind("reservation_corrupt")
            .bind("")
            .bind(10_900_i64)
            .bind(8_i64)
            .execute(&pool)
            .await,
        );
        pool.close().await;
    });
    let state_bytes_before_status = must_ok(fs::read(&state_path));

    let output = run_cli(
        [
            "codex-router",
            "quota",
            "status",
            "--router-root",
            path_to_str(&router_root),
            "--format",
            "json",
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );

    let parsed: serde_json::Value = must_ok(serde_json::from_str(&output.stdout));
    assert_eq!(parsed["route_result"], "degraded");
    assert_eq!(
        parsed["selection_projection_source"],
        "display_windows_fallback"
    );
    assert_eq!(
        parsed["preferred_next_account_hash"],
        serde_json::Value::Null
    );
    assert_eq!(
        parsed["accounts"][0]["account_hash"].as_str().map(str::len),
        Some(16)
    );
    assert!(!output.stdout.contains("acct_primary"));
    assert_eq!(
        parsed["accounts"][0]["active_clients"],
        serde_json::Value::Null
    );
    assert_eq!(
        parsed["accounts"][0]["active_clients_source"],
        "unavailable"
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["5h"]["remaining_headroom"],
        100
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["weekly"]["remaining_headroom"],
        20
    );
    assert_eq!(parsed["accounts"][0]["preferred_next"], false);
    assert_eq!(parsed["accounts"][0]["freshness"], "fresh");
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["weekly"]["evidence_state"],
        "known"
    );
    assert_eq!(
        parsed["accounts"][0]["window_slots"]["weekly"]["run_rate"]["confidence"],
        "insufficient"
    );
    assert_eq!(
        parsed["accounts"][0]["weekly_quota_floor_basis_points"],
        500
    );
    assert_eq!(parsed["accounts"][0]["weekly_quota_floor_percent"], 5);
    assert_eq!(must_ok(fs::read(&state_path)), state_bytes_before_status);
    assert_eq!(parsed["accounts"][0]["next_use"], "fallback by quota");
    assert_eq!(
        parsed["accounts"][0]["routing_reason"],
        "unknown_fallback_available"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn quota_status_plain_uses_persisted_history_for_run_rate() {
    let test_root = TestRoot::new("quota-status-history");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state_path = router_root.join("state.sqlite");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let primary_account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id("acct_primary_history"),
        "primary",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &primary_account,
    ));
    let five_hour_reset = 20_000;
    let five_hour_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary_history"),
        "responses",
        18_000,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(80)
    .with_reset_unix_seconds(five_hour_reset)
    .with_effective(true)
    .with_observed_unix_seconds(11_000);
    let weekly_window = PersistedSelectorQuotaWindow::new(
        account_id("acct_primary_history"),
        "responses",
        604_800,
        SelectorQuotaWindowStatus::Eligible,
    )
    .with_remaining_headroom(90)
    .with_reset_unix_seconds(614_800)
    .with_observed_unix_seconds(11_000);
    must_ok(
        SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
            &state,
            primary_account.account_id(),
            "responses",
            &[five_hour_window, weekly_window],
            11_000,
            five_hour_reset,
        ),
    );
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let async_state = must_ok(runtime.block_on(AsyncSqliteStateStore::open(&state_path)));
    for (observed_unix_seconds, remaining_headroom) in [
        (10_000_u64, 90_u32),
        (10_500_u64, 85_u32),
        (11_000_u64, 80_u32),
    ] {
        let observation = PersistedQuotaHistoryObservation::new(
            account_id("acct_primary_history"),
            "primary",
            "responses",
            18_000,
            observed_unix_seconds,
            remaining_headroom,
        )
        .with_reset_unix_seconds(five_hour_reset)
        .with_effective(true);
        must_ok(runtime.block_on(async_state.append_quota_history_observation(&observation)));
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
            "--no-refresh",
            "--now-unix-seconds",
            "11000",
        ],
        CliContext::new(Vec::new()),
    );

    assert!(
        output.stdout.contains("reset pace") || output.stdout.contains("burn unavailable"),
        "{}",
        output.stdout
    );
    assert!(output.stdout.contains("sample fresh"), "{}", output.stdout);
    assert!(!output.stdout.contains("normal burn"), "{}", output.stdout);
    assert!(
        !output.stdout.contains("history unknown"),
        "{}",
        output.stdout
    );
    assert!(output.stderr.is_empty());
}
