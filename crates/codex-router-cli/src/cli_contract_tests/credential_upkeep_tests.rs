use super::*;

use crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock;
use codex_router_core::provider::Provider;
use codex_router_secret_store::credential_bundle::CredentialBundle;

#[derive(Clone)]
struct RecordingUpkeepRefreshClient {
    calls: Arc<AtomicUsize>,
    observed_calls: mpsc::Sender<usize>,
}

impl CredentialRefreshClient for RecordingUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        assert_eq!(account_id.as_str(), "upkeep-enabled");
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(
            refresh_token.expose_secret(),
            if call == 1 {
                "initial-refresh-canary"
            } else {
                "rotated-refresh-canary"
            }
        );
        self.observed_calls
            .send(call)
            .expect("provider call should be observed");
        Ok(AccountCredentialBundle::imported_codex_auth(
            format!("replacement-access-{call}"),
            Some("rotated-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(10_000_000))
    }
}

#[test]
fn enabled_exhausted_idle_account_renews_across_simulated_days_without_quota_probe() {
    let test_root = TestRoot::new("credential-upkeep-idle");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let enabled_id = account_id("upkeep-enabled");
    let disabled_id = account_id("upkeep-disabled");
    for (account_id, status) in [
        (enabled_id.clone(), AccountStatus::Enabled),
        (disabled_id.clone(), AccountStatus::Disabled),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                "upkeep",
                status,
            )
            .with_active_credential_generation(1),
        ));
        let key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
        must_ok(
            secrets.write_secret(
                &key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "initial-access-canary",
                        Some("initial-refresh-canary".to_owned()),
                    )
                    .with_expires_unix_seconds(10_000_000)
                    .to_secret_string(),
                ),
            ),
        );
    }
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(enabled_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("responses", 0),
    ));
    drop(state);

    let now = Arc::new(AtomicU64::new(1_000));
    let (call_sender, call_receiver) = mpsc::channel();
    let calls = Arc::new(AtomicUsize::new(0));
    let client = RecordingUpkeepRefreshClient {
        calls: Arc::clone(&calls),
        observed_calls: call_sender,
    };
    let clock = Arc::clone(&now);
    let worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            &state_path,
            secrets.into(),
            client,
            move || clock.load(Ordering::SeqCst),
        ),
    );
    assert_eq!(
        must_ok(call_receiver.recv_timeout(Duration::from_secs(2))),
        1
    );
    wait_for_upkeep_generation(&state_path, &enabled_id, 2);
    now.store(1_000 + 2 * 86_400, Ordering::SeqCst);
    worker.wake_for_test();
    assert_eq!(
        must_ok(call_receiver.recv_timeout(Duration::from_secs(2))),
        2
    );
    wait_for_upkeep_generation(&state_path, &enabled_id, 3);
    drop(worker);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let read_runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let read_state =
        must_ok(read_runtime.block_on(AsyncSqliteStateStore::open_read_only(&state_path)));
    let disabled = must_ok(read_runtime.block_on(read_state.load_account(&disabled_id)))
        .expect("disabled account should remain");
    assert_eq!(disabled.status(), AccountStatus::Disabled);
    assert_eq!(disabled.active_credential_generation(), Some(1));
}

#[derive(Clone)]
struct RecordingClaudeUpkeepRefreshClient {
    observed_account_ids: mpsc::Sender<AccountId>,
}

impl CredentialRefreshClient for RecordingClaudeUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        Err(codex_router_auth::resolver::CredentialRefreshFailure::ambiguous(
            codex_router_state::credential_maintenance::CredentialFailureClass::ProviderOutcomeAmbiguous,
        ))
    }

    fn refresh_provider_credentials(
        &self,
        provider: Provider,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<CredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure> {
        assert_eq!(provider, Provider::Claude);
        assert_eq!(
            refresh_token.expose_secret(),
            format!("claude-refresh-{}", account_id.as_str())
        );
        self.observed_account_ids
            .send(account_id.clone())
            .expect("Claude refresh call should be observed");
        CredentialBundle::new_claude(
            SecretString::new(format!("claude-access-{}", account_id.as_str())),
            SecretString::new(format!("claude-rotated-{}", account_id.as_str())),
            10_000_000,
        )
        .map_err(|_| {
            codex_router_auth::resolver::CredentialRefreshFailure::ambiguous(
                codex_router_state::credential_maintenance::CredentialFailureClass::MalformedResponse,
            )
        })
    }
}

#[test]
fn credential_upkeep_refreshes_exhausted_and_idle_claude_accounts() {
    let test_root = TestRoot::new("credential-upkeep-exhausted-idle-claude");
    must_ok(fs::create_dir(test_root.path()));
    let router_root = test_root.path().join("router");
    must_ok(fs::create_dir(&router_root));
    ensure_async_state_schema(&router_root);
    let state_path = router_root.join("state.sqlite");
    let secret_root = router_root.join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let exhausted_id = account_id("upkeep-claude-exhausted");
    let idle_id = account_id("upkeep-claude-idle");

    for (account_id, label) in [
        (&exhausted_id, "exhausted Claude"),
        (&idle_id, "idle Claude"),
    ] {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let credential_key = must_ok(
            codex_router_secret_store::account_tokens::provider_credential_bundle_key(
                Provider::Claude,
                account_id,
                1,
            ),
        );
        let bundle = must_ok(CredentialBundle::new_claude(
            SecretString::new(format!("claude-access-{}", account_id.as_str())),
            SecretString::new(format!("claude-refresh-{}", account_id.as_str())),
            10_000_000,
        ));
        must_ok(secrets.write_secret(&credential_key, &must_ok(bundle.to_secret_string())));
    }
    must_ok(QuotaSnapshotRepository::upsert_snapshot(
        &state,
        &PersistedQuotaSnapshot::new(exhausted_id.clone(), QuotaSnapshotSource::MockEndpoint)
            .with_observed_unix_seconds(1_000)
            .with_route_band("claude_messages", 0),
    ));
    drop(state);

    let (observed_sender, observed_receiver) = mpsc::channel();
    let worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            &state_path,
            secrets.clone().into(),
            RecordingClaudeUpkeepRefreshClient {
                observed_account_ids: observed_sender,
            },
            || 1_000,
        ),
    );
    let mut observed_accounts = [
        must_ok(observed_receiver.recv_timeout(Duration::from_secs(2)))
            .as_str()
            .to_owned(),
        must_ok(observed_receiver.recv_timeout(Duration::from_secs(2)))
            .as_str()
            .to_owned(),
    ];
    observed_accounts.sort();
    let mut expected_accounts = [
        exhausted_id.as_str().to_owned(),
        idle_id.as_str().to_owned(),
    ];
    expected_accounts.sort();
    assert_eq!(observed_accounts, expected_accounts);
    wait_for_upkeep_generation(&state_path, &exhausted_id, 2);
    wait_for_upkeep_generation(&state_path, &idle_id, 2);
    drop(worker);

    for account_id in [&exhausted_id, &idle_id] {
        let credential_key = must_ok(
            codex_router_secret_store::account_tokens::provider_credential_bundle_key(
                Provider::Claude,
                account_id,
                2,
            ),
        );
        let stored = must_ok(secrets.read_secret(&credential_key));
        let refreshed = must_ok(CredentialBundle::from_secret_string(
            Provider::Claude,
            stored,
        ));
        assert_eq!(refreshed.provider(), Provider::Claude);
        assert_eq!(
            refreshed
                .refresh_token()
                .expect("Claude account remains renewable")
                .expose_secret(),
            format!("claude-rotated-{}", account_id.as_str())
        );
    }
}

#[derive(Clone)]
struct HeldUpkeepRefreshClient {
    entered_sender: mpsc::Sender<()>,
    release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        self.entered_sender
            .send(())
            .expect("provider entry should report");
        self.release_receiver
            .lock()
            .expect("release lock")
            .recv_timeout(Duration::from_secs(2))
            .expect("provider should be released");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(10_000))
    }
}

#[test]
fn upkeep_shutdown_drains_in_flight_rotation_before_returning() {
    let test_root = TestRoot::new("credential-upkeep-shutdown");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("upkeep-shutdown");
    must_ok(AccountStateRepository::upsert_account(
        &state,
        &AccountRecord::new(
            codex_router_core::provider::Provider::Openai,
            account_id.clone(),
            "shutdown",
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "initial-access-canary",
                    Some("initial-refresh-canary".to_owned()),
                )
                .with_expires_unix_seconds(1_100)
                .to_secret_string(),
            ),
        ),
    );
    drop(state);
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            &state_path,
            secrets.into(),
            HeldUpkeepRefreshClient {
                entered_sender,
                release_receiver: Arc::new(Mutex::new(release_receiver)),
            },
            || 1_000,
        ),
    );
    must_ok(entered_receiver.recv_timeout(Duration::from_secs(2)));
    let (stopped_sender, stopped_receiver) = mpsc::channel();
    let shutdown = thread::spawn(move || {
        drop(worker);
        stopped_sender.send(()).expect("shutdown should report");
    });
    assert!(matches!(
        stopped_receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    must_ok(release_sender.send(()));
    must_ok(stopped_receiver.recv_timeout(Duration::from_secs(2)));
    must_ok(shutdown.join().map_err(|_| "shutdown thread failed"));
    wait_for_upkeep_generation(&state_path, &account_id, 2);
}

#[derive(Clone)]
struct HeldQueuedUpkeepRefreshClient {
    entered_sender: mpsc::Sender<AccountId>,
    release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldQueuedUpkeepRefreshClient {
    fn refresh_credentials(
        &self,
        account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        self.entered_sender
            .send(account_id.clone())
            .expect("provider entry should report account");
        self.release_receiver
            .lock()
            .expect("release lock")
            .recv_timeout(Duration::from_secs(3))
            .expect("claimed provider use should be released");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(10_000))
    }
}

#[test]
fn upkeep_shutdown_does_not_admit_a_queued_account_after_stop() {
    let test_root = TestRoot::new("credential-upkeep-queued-stop");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_ids = (0..5)
        .map(|index| account_id(&format!("queued-upkeep-{index}")))
        .collect::<Vec<_>>();
    for (index, account_id) in account_ids.iter().enumerate() {
        must_ok(AccountStateRepository::upsert_account(
            &state,
            &AccountRecord::new(
                codex_router_core::provider::Provider::Openai,
                account_id.clone(),
                format!("queued-{index}"),
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        ));
        let active_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
        must_ok(
            secrets.write_secret(
                &active_key,
                &must_ok(
                    AccountCredentialBundle::imported_codex_auth(
                        "initial-access-canary",
                        Some("initial-refresh-canary".to_owned()),
                    )
                    .with_expires_unix_seconds(1_100)
                    .to_secret_string(),
                ),
            ),
        );
    }
    drop(state);
    let credential_store = secrets.clone();
    drop(secrets);

    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            &state_path,
            credential_store.into(),
            HeldQueuedUpkeepRefreshClient {
                entered_sender,
                release_receiver: Arc::new(Mutex::new(release_receiver)),
            },
            || 1_000,
        ),
    );
    let admitted = (0..4)
        .map(|_| must_ok(entered_receiver.recv_timeout(Duration::from_secs(3))))
        .collect::<Vec<_>>();
    let (stopped_sender, stopped_receiver) = mpsc::channel();
    let shutdown = thread::spawn(move || {
        drop(worker);
        stopped_sender.send(()).expect("shutdown should report");
    });
    assert!(matches!(
        stopped_receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    for _ in 0..4 {
        must_ok(release_sender.send(()));
    }
    let post_stop_entry = entered_receiver.recv_timeout(Duration::from_secs(2)).ok();
    if post_stop_entry.is_some() {
        must_ok(release_sender.send(()));
    }
    must_ok(stopped_receiver.recv_timeout(Duration::from_secs(3)));
    must_ok(shutdown.join().map_err(|_| "shutdown thread failed"));
    assert!(
        post_stop_entry.is_none(),
        "queued account reached provider after Stop: {post_stop_entry:?}"
    );
    for account_id in &admitted {
        wait_for_upkeep_generation(&state_path, account_id, 2);
    }
    let queued_id = account_ids
        .iter()
        .find(|account_id| !admitted.contains(account_id))
        .expect("one account should remain queued");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let queued = must_ok(AccountStateRepository::load_account(&state, queued_id))
        .expect("queued account should remain");
    assert_eq!(queued.active_credential_generation(), Some(1));
}

pub(super) fn wait_for_upkeep_generation(state_path: &Path, account_id: &AccountId, expected: u64) {
    let runtime = must_ok(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build(),
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let state = loop {
        match runtime.block_on(AsyncSqliteStateStore::open_read_only(state_path)) {
            Ok(state) => break state,
            Err(error) if format!("{error}").contains("locked") && Instant::now() < deadline => {
                thread::yield_now();
            }
            Err(error) => panic!("read-only upkeep observation failed: {error}"),
        }
    };
    loop {
        match runtime.block_on(state.load_account(account_id)) {
            Ok(Some(account)) if account.active_credential_generation() == Some(expected) => {
                must_ok(runtime.block_on(state.close()));
                return;
            }
            Ok(_) => {}
            Err(error) if format!("{error}").contains("locked") => {}
            Err(error) => panic!("upkeep account observation failed: {error}"),
        }
        assert!(Instant::now() < deadline, "credential activation timed out");
        thread::yield_now();
    }
}
