use super::*;

#[tokio::test]
async fn background_quota_refresh_worker_runs_immediate_cycle_without_waiting_for_interval() {
    let test_root = TestRoot::new("background-quota-refresh-immediate");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_background_refresh");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "background",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "background-access-token",
                    Some("background-refresh-token".to_owned()),
                )
                .to_secret_string(),
            ),
        ),
    );
    let resolver = must_ok(
        crate::credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            NoopCredentialRefreshClient,
        )
        .await,
    );
    let (refresh_sender, mut refresh_receiver) = tokio::sync::mpsc::unbounded_channel();
    let provider = SignalingQuotaRefreshProvider::new(58, refresh_sender);

    let mut worker = start_background_quota_refresh_worker_with_dependencies(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        Duration::from_secs(3_600),
    )
    .await;

    let observed_route_band = tokio::time::timeout(Duration::from_secs(2), refresh_receiver.recv())
        .await
        .unwrap_or_else(|error| panic!("background refresh should run immediately: {error}"))
        .expect("background refresh route band should be reported");
    assert_eq!(observed_route_band, "responses");
    worker.shutdown().await;
    let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
        &state,
        &account_id,
        "responses",
    ))
    .unwrap_or_else(|| panic!("background refresh should persist snapshot"));
    assert_eq!(snapshot.remaining_headroom(), 58);
    assert!(snapshot.observed_unix_seconds() > 0);
}

#[tokio::test]
async fn background_quota_refresh_worker_start_does_not_wait_for_slow_provider() {
    let test_root = TestRoot::new("background-quota-refresh-slow-provider");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_background_refresh_slow_provider");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "background",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "background-slow-access-token",
                    Some("background-slow-refresh-token".to_owned()),
                )
                .to_secret_string(),
            ),
        ),
    );
    let resolver = must_ok(
        crate::credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            NoopCredentialRefreshClient,
        )
        .await,
    );
    let provider = SlowQuotaRefreshProvider::new(Duration::from_millis(500), 72);
    let cycle_count = Arc::new(AtomicUsize::new(0));
    let observed_cycle_count = Arc::clone(&cycle_count);
    let worker_runtime = BackgroundQuotaRefreshRuntime::new(
        move || {
            observed_cycle_count.fetch_add(1, Ordering::SeqCst);
            1_300
        },
        |_diagnostic| {},
        Duration::ZERO,
    );

    let start = Instant::now();
    let mut worker = start_background_quota_refresh_worker_with_reporter(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        worker_runtime,
    )
    .await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_millis(250),
        "background worker startup waited for provider: {elapsed:?}"
    );
    worker.shutdown().await;
    assert_eq!(
        cycle_count.load(Ordering::SeqCst),
        1,
        "a zero background interval performs exactly one immediate cycle"
    );
    tokio::time::timeout(
        Duration::from_secs(1),
        tokio::time::sleep(Duration::from_millis(1)),
    )
    .await
    .expect("caller runtime should still schedule after quota-worker shutdown");
}

#[tokio::test]
async fn background_quota_refresh_worker_uses_fresh_time_for_each_cycle() {
    let test_root = TestRoot::new("background-quota-refresh-fresh-time");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_background_refresh_fresh_time");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "background",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "background-fresh-access-token",
                    Some("background-fresh-refresh-token".to_owned()),
                )
                .to_secret_string(),
            ),
        ),
    );
    let resolver = must_ok(
        crate::credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            NoopCredentialRefreshClient,
        )
        .await,
    );
    let (refresh_sender, mut refresh_receiver) = tokio::sync::mpsc::unbounded_channel();
    let provider = SignalingQuotaRefreshProvider::new(64, refresh_sender);
    let clock = Arc::new(AtomicU64::new(1_300));
    let worker_clock = Arc::clone(&clock);

    let mut worker = start_background_quota_refresh_worker_with_clock(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        move || worker_clock.fetch_add(5, Ordering::SeqCst),
        Duration::from_millis(1),
    )
    .await;

    for _call_index in 0..4 {
        tokio::time::timeout(Duration::from_secs(2), refresh_receiver.recv())
            .await
            .unwrap_or_else(|error| {
                panic!("background refresh should run multiple cycles: {error}")
            })
            .expect("background refresh cycle should report its route band");
    }
    worker.shutdown().await;
    let snapshot = must_ok(QuotaSnapshotRepository::load_snapshot_for_route_band(
        &state,
        &account_id,
        "responses",
    ))
    .unwrap_or_else(|| panic!("background refresh should persist snapshot"));
    assert_eq!(snapshot.remaining_headroom(), 64);
    assert!(snapshot.observed_unix_seconds() > 1_300);
}

#[test]
fn background_quota_refresh_cycle_delay_subtracts_elapsed_work() {
    assert_eq!(
        crate::quota::refresh_cycle_delay(Duration::from_secs(180), Duration::from_secs(17),),
        Duration::from_secs(163)
    );
    assert_eq!(
        crate::quota::refresh_cycle_delay(Duration::from_secs(180), Duration::from_secs(181),),
        Duration::ZERO
    );
}

#[tokio::test]
async fn background_quota_refresh_worker_reports_refresh_failures() {
    let test_root = TestRoot::new("background-quota-refresh-diagnostics");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let account_id = account_id("acct_background_refresh_diagnostics");
    let unsafe_account_label = "person@example.com";
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        unsafe_account_label,
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let bundle_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &bundle_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "background-diagnostic-token-canary",
                    Some("background-diagnostic-refresh-token".to_owned()),
                )
                .to_secret_string(),
            ),
        ),
    );
    let resolver = must_ok(
        crate::credential_runtime::AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            NoopCredentialRefreshClient,
        )
        .await,
    );
    let provider = AccountFailingQuotaRefreshProvider::new(unsafe_account_label, 429, 0);
    let (diagnostic_sender, mut diagnostic_receiver) = tokio::sync::mpsc::unbounded_channel();

    let mut worker = start_background_quota_refresh_worker_with_reporter(
        state_path,
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        provider,
        BackgroundQuotaRefreshRuntime::new(
            || 1_300,
            move |diagnostic| {
                if let Err(error) = diagnostic_sender.send(diagnostic) {
                    panic!("background diagnostic should send: {error}");
                }
            },
            Duration::from_secs(0),
        ),
    )
    .await;

    let first_diagnostic = tokio::time::timeout(Duration::from_secs(2), diagnostic_receiver.recv())
        .await
        .unwrap_or_else(|error| panic!("background refresh should report route failures: {error}"))
        .expect("route failure diagnostic should send");
    let second_diagnostic =
        tokio::time::timeout(Duration::from_secs(2), diagnostic_receiver.recv())
            .await
            .unwrap_or_else(|error| {
                panic!("background refresh should report command failure: {error}")
            })
            .expect("command failure diagnostic should send");
    worker.shutdown().await;
    let diagnostics = format!("{first_diagnostic}\n{second_diagnostic}");
    assert!(diagnostics.contains("refresh failed: account=acct-"));
    assert!(!diagnostics.contains(unsafe_account_label));
    assert!(diagnostics.contains("failed: 2"));
    assert!(diagnostics.contains(
            "background quota refresh failed: quota refresh provider response was unusable: quota refresh failed for all eligible route bands"
        ));
    assert!(!diagnostics.contains("background-diagnostic-token-canary"));
}

#[test]
fn cli_credential_resolver_refreshes_expired_bundle_through_runtime_wrapper() {
    let observed_now = must_ok(codex_router_auth::resolver::current_unix_seconds());
    let test_root = TestRoot::new("cli-runtime-resolver-refresh");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir_all(&router_root));
    let state = must_ok(SqliteStateStore::open(&router_root.join("state.sqlite")));
    let account_id = account_id("acct_cli_runtime_refresh");
    let account = AccountRecord::new(
        codex_router_core::provider::Provider::Openai,
        account_id.clone(),
        "runtime",
        AccountStatus::Enabled,
    )
    .with_active_credential_generation(1);
    must_ok(AccountStateRepository::upsert_account(&state, &account));
    let secrets_root = router_root.join("secrets");
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secrets_root),
    );
    let expired_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &expired_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-cli-runtime-access-token",
                    Some("cli-runtime-refresh-token".to_owned()),
                )
                .with_expires_unix_seconds(observed_now.saturating_sub(100))
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new(
        "acct_cli_runtime_refresh",
        "cli-runtime-refresh-token",
        AccountCredentialBundle::imported_codex_auth(
            "refreshed-cli-runtime-access-token",
            Some("refreshed-cli-runtime-refresh-token".to_owned()),
        )
        .with_expires_unix_seconds(observed_now.saturating_add(3_600)),
    );
    let resolver = must_ok(CliCredentialResolver::open_with_refresh_client(
        &router_root.join("state.sqlite"),
        &secrets_root,
        refresh_client.clone(),
    ));

    let resolved =
        must_ok(resolver.resolve_provider_credentials(
            &account_id,
            codex_router_core::provider::Provider::Openai,
        ));

    assert_eq!(
        resolved.access_token().expose_secret(),
        "refreshed-cli-runtime-access-token"
    );
    assert_eq!(refresh_client.calls(), 1);
    let loaded_account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .unwrap_or_else(|| panic!("account should remain registered"));
    assert_eq!(loaded_account.active_credential_generation(), Some(2));
}
