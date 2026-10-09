use super::*;

use crate::credential_runtime::AsyncCliCredentialResolver;
use crate::credential_upkeep_worker::start_background_credential_upkeep_worker_with_client_and_clock;
use crate::quota::start_background_quota_refresh_worker_with_clock;
use codex_router_core::provider::Provider;
use codex_router_secret_store::account_tokens::openai_account_credential_bundle_key;

#[derive(Clone)]
struct HeldProducerRefreshClient {
    expected_account_id: AccountId,
    expected_refresh_token: &'static str,
    calls: Arc<AtomicUsize>,
    entered_sender: tokio::sync::mpsc::UnboundedSender<AccountId>,
    release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl CredentialRefreshClient for HeldProducerRefreshClient {
    fn refresh_credentials(
        &self,
        account_id: &AccountId,
        refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, codex_router_auth::resolver::CredentialRefreshFailure>
    {
        assert_eq!(
            self.calls.fetch_add(1, Ordering::SeqCst),
            0,
            "the fixture must never spend one refresh token twice"
        );
        assert_eq!(account_id, &self.expected_account_id);
        assert_eq!(refresh_token.expose_secret(), self.expected_refresh_token);
        self.entered_sender
            .send(account_id.clone())
            .expect("the producer should report its admitted refresh");
        self.release_receiver
            .lock()
            .expect("refresh release lock")
            .recv_timeout(Duration::from_secs(5))
            .expect("the test should release the admitted refresh");
        Ok(AccountCredentialBundle::imported_codex_auth(
            format!("{}-replacement-access", account_id.as_str()),
            Some(format!("{}-replacement-refresh", account_id.as_str())),
        )
        .with_expires_unix_seconds(10_000_000))
    }
}

fn held_producer_refresh_client(
    account_id: AccountId,
    expected_refresh_token: &'static str,
) -> (
    HeldProducerRefreshClient,
    tokio::sync::mpsc::UnboundedReceiver<AccountId>,
    mpsc::Sender<()>,
) {
    let (entered_sender, entered_receiver) = tokio::sync::mpsc::unbounded_channel();
    let (release_sender, release_receiver) = mpsc::channel();
    (
        HeldProducerRefreshClient {
            expected_account_id: account_id,
            expected_refresh_token,
            calls: Arc::new(AtomicUsize::new(0)),
            entered_sender,
            release_receiver: Arc::new(Mutex::new(release_receiver)),
        },
        entered_receiver,
        release_sender,
    )
}

struct UnauthorizedQuotaRefreshProvider {
    calls: Arc<AtomicUsize>,
    entered_sender: tokio::sync::mpsc::UnboundedSender<()>,
}

impl QuotaRefreshProvider for UnauthorizedQuotaRefreshProvider {
    async fn fetch_quota(
        &self,
        _request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, QuotaCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered_sender
            .send(())
            .expect("401 quota attempt should be observed");
        Err(QuotaCommandError::ProviderStatus { status: 401 })
    }
}

fn add_expired_openai_account(
    state: &SqliteStateStore,
    secrets: &impl SecretStore,
    account_id: &AccountId,
    label: &str,
) {
    add_openai_account_with_expiry(state, secrets, account_id, label, 1);
}

fn add_openai_account_with_expiry(
    state: &SqliteStateStore,
    secrets: &impl SecretStore,
    account_id: &AccountId,
    label: &str,
    expires_unix_seconds: u64,
) {
    must_ok(AccountStateRepository::upsert_account(
        state,
        &AccountRecord::new(
            Provider::Openai,
            account_id.clone(),
            label,
            AccountStatus::Enabled,
        )
        .with_active_credential_generation(1),
    ));
    let credential_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
    must_ok(
        secrets.write_secret(
            &credential_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    format!("{}-expired-access", account_id.as_str()),
                    Some(format!("{}-original-refresh", account_id.as_str())),
                )
                .with_expires_unix_seconds(expires_unix_seconds)
                .to_secret_string(),
            ),
        ),
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_request_supervisor_rejects_late_upkeep_and_quota_renewals() {
    let test_root = TestRoot::new("shared-renewal-supervisor-counterexample");
    must_ok(fs::create_dir(test_root.path()));
    let upkeep_state_path = test_root.path().join("upkeep.sqlite");
    let quota_state_path = test_root.path().join("quota.sqlite");
    let secret_root = test_root.path().join("secrets");
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let upkeep_account_id = account_id("shared-close-upkeep");
    let quota_account_id = account_id("shared-close-quota");
    let upkeep_state = must_ok(SqliteStateStore::open(&upkeep_state_path));
    add_expired_openai_account(
        &upkeep_state,
        &secrets,
        &upkeep_account_id,
        "upkeep account",
    );
    drop(upkeep_state);
    let quota_state = must_ok(SqliteStateStore::open(&quota_state_path));
    add_expired_openai_account(&quota_state, &secrets, &quota_account_id, "quota account");
    drop(quota_state);

    let runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
        must_ok(LoopbackBindAddress::new(
            "127.0.0.1",
            reserve_loopback_port(),
        )),
        must_ok(UpstreamEndpoint::new("http://127.0.0.1:1/v1")),
        upkeep_state_path.clone(),
        secret_root.clone(),
    );
    let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config, secrets.clone()).await);
    let refresh_tasks = runtime.credential_refresh_task_supervisor();
    assert!(matches!(runtime.serve_protocol_connections(0).await, Ok(0)));
    drop(runtime);

    let (upkeep_client, mut upkeep_entered, upkeep_release) = held_producer_refresh_client(
        upkeep_account_id.clone(),
        "shared-close-upkeep-original-refresh",
    );
    let upkeep_refresh_calls = Arc::clone(&upkeep_client.calls);
    let (upkeep_cycle_sender, mut upkeep_cycle_receiver) = tokio::sync::mpsc::unbounded_channel();
    let upkeep_worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            upkeep_state_path.clone(),
            secrets.clone(),
            refresh_tasks.clone(),
            upkeep_client,
            move || {
                upkeep_cycle_sender
                    .send(())
                    .expect("upkeep scheduler should report each cycle");
                1_000
            },
        )
        .await,
    );
    tokio::time::timeout(Duration::from_secs(3), upkeep_cycle_receiver.recv())
        .await
        .expect("upkeep scheduler should start its first cycle")
        .expect("upkeep scheduler should report its first cycle");
    let _ = upkeep_release.send(());
    upkeep_worker.wake_for_test();
    tokio::time::timeout(Duration::from_secs(3), upkeep_cycle_receiver.recv())
        .await
        .expect("upkeep should finish the closed-admission cycle and honor its wake")
        .expect("upkeep scheduler should report its second cycle");

    let (quota_client, mut quota_entered, quota_release) = held_producer_refresh_client(
        quota_account_id.clone(),
        "shared-close-quota-original-refresh",
    );
    let quota_refresh_calls = Arc::clone(&quota_client.calls);
    let quota_resolver = must_ok(
        AsyncCliCredentialResolver::open_with_refresh_client(
            &quota_state_path,
            secrets.clone(),
            quota_client,
            refresh_tasks.clone(),
        )
        .await,
    );
    let (quota_cycle_sender, mut quota_cycle_receiver) = tokio::sync::mpsc::unbounded_channel();
    let quota_worker = start_background_quota_refresh_worker_with_clock(
        quota_state_path.clone(),
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        quota_resolver,
        StaticQuotaRefreshProvider::new(verified_quota_windows(58)),
        move || {
            quota_cycle_sender
                .send(())
                .expect("quota scheduler should report its cycle");
            1_000
        },
        Duration::ZERO,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), quota_cycle_receiver.recv())
        .await
        .expect("quota scheduler should start its cycle")
        .expect("quota scheduler should report its cycle");
    let _ = quota_release.send(());
    let mut upkeep_worker = upkeep_worker;
    upkeep_worker.shutdown().await;
    let mut quota_worker = quota_worker;
    quota_worker.shutdown().await;
    assert!(
        upkeep_entered.try_recv().is_err(),
        "closed request supervisor must reject upkeep before provider egress"
    );
    assert!(
        quota_entered.try_recv().is_err(),
        "closed request supervisor must reject quota before provider egress"
    );
    assert!(
        refresh_tasks
            .wait_for_completion(Duration::from_secs(1))
            .await
    );
    assert_eq!(upkeep_refresh_calls.load(Ordering::SeqCst), 0);
    assert_eq!(quota_refresh_calls.load(Ordering::SeqCst), 0);

    for (state_path, account_id) in [
        (&upkeep_state_path, &upkeep_account_id),
        (&quota_state_path, &quota_account_id),
    ] {
        let state = must_ok(SqliteStateStore::open(state_path));
        let account = must_ok(AccountStateRepository::load_account(&state, account_id))
            .expect("producer account should remain in state");
        assert_eq!(account.active_credential_generation(), Some(1));
        let original_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
        let original = must_ok(secrets.read_secret(&original_key));
        let original = must_ok(AccountCredentialBundle::from_secret_string(original));
        assert_eq!(
            original
                .refresh_token()
                .expect("original bundle should retain its refresh token")
                .expose_secret(),
            format!("{}-original-refresh", account_id.as_str())
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_close_rejects_quota_and_retains_upkeep_claim_to_successor() {
    let test_root = TestRoot::new("shared-renewal-supervisor-cross-producer");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("shared-producer-account");
    add_expired_openai_account(&state, &secrets, &account_id, "shared producer");
    drop(state);

    let runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
        must_ok(LoopbackBindAddress::new(
            "127.0.0.1",
            reserve_loopback_port(),
        )),
        must_ok(UpstreamEndpoint::new("http://127.0.0.1:1/v1")),
        state_path.clone(),
        secret_root.clone(),
    );
    let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config, secrets.clone()).await);
    let refresh_tasks = runtime.credential_refresh_task_supervisor();
    let (refresh_client, mut provider_entries, release_provider) = held_producer_refresh_client(
        account_id.clone(),
        "shared-producer-account-original-refresh",
    );
    let provider_call_count = Arc::clone(&refresh_client.calls);
    let upkeep_worker = must_ok(
        start_background_credential_upkeep_worker_with_client_and_clock(
            state_path.clone(),
            secrets.clone(),
            refresh_tasks.clone(),
            refresh_client.clone(),
            || 1_000,
        )
        .await,
    );
    let first_provider_entry =
        tokio::time::timeout(Duration::from_secs(3), provider_entries.recv())
            .await
            .expect("upkeep should reach the provider after its durable claim")
            .expect("upkeep should report its held provider call");
    assert_eq!(first_provider_entry, account_id);

    let quota_resolver = must_ok(
        AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            refresh_client,
            refresh_tasks.clone(),
        )
        .await,
    );
    let (quota_cycle_sender, mut quota_cycle_receiver) = tokio::sync::mpsc::unbounded_channel();
    let quota_worker = start_background_quota_refresh_worker_with_clock(
        state_path.clone(),
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        quota_resolver,
        StaticQuotaRefreshProvider::new(verified_quota_windows(58)),
        move || {
            quota_cycle_sender
                .send(())
                .expect("quota scheduler should report its active cycle");
            1_000
        },
        Duration::ZERO,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), quota_cycle_receiver.recv())
        .await
        .expect("quota producer should enter its active cycle")
        .expect("quota producer should report its active cycle");

    refresh_tasks.close_admission();
    assert!(
        !refresh_tasks
            .wait_for_completion(Duration::from_millis(25))
            .await,
        "bounded wait must leave the post-claim upkeep refresh tracked"
    );
    let mut quota_worker = quota_worker;
    quota_worker.shutdown().await;
    assert_eq!(provider_call_count.load(Ordering::SeqCst), 1);
    assert!(
        provider_entries.try_recv().is_err(),
        "quota must not spend the same refresh token after close"
    );

    must_ok(release_provider.send(()));
    assert!(
        refresh_tasks
            .wait_for_completion(Duration::from_secs(2))
            .await
    );
    let mut upkeep_worker = upkeep_worker;
    upkeep_worker.shutdown().await;
    runtime.shutdown().await;
    drop(runtime);

    let state = must_ok(AsyncSqliteStateStore::open_read_only(&state_path).await);
    let account = must_ok(state.load_account(&account_id).await)
        .expect("shared producer account should remain durable");
    assert_eq!(account.active_credential_generation(), Some(2));
    let maintenance = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("successful renewal disposition should remain durable");
    assert_eq!(
        maintenance.state,
        codex_router_state::credential_maintenance::CredentialMaintenanceState::Healthy
    );
    must_ok(state.close().await);
    let successor_key = must_ok(openai_account_credential_bundle_key(&account_id, 2));
    let successor = must_ok(secrets.read_secret(&successor_key));
    let successor = must_ok(AccountCredentialBundle::from_secret_string(successor));
    assert_eq!(
        successor
            .refresh_token()
            .expect("successor should remain renewable")
            .expose_secret(),
        "shared-producer-account-replacement-refresh"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closed_supervisor_rejects_quota_401_recovery_without_spending_refresh_token() {
    let test_root = TestRoot::new("shared-renewal-supervisor-quota-401");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let secret_root = test_root.path().join("secrets");
    let state = must_ok(SqliteStateStore::open(&state_path));
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root),
    );
    let account_id = account_id("closed-quota-401-account");
    add_openai_account_with_expiry(
        &state,
        &secrets,
        &account_id,
        "closed quota 401",
        4_000_000_000,
    );
    drop(state);

    let runtime_config = LoopbackRouterRuntimeConfig::new_tokenless(
        must_ok(LoopbackBindAddress::new(
            "127.0.0.1",
            reserve_loopback_port(),
        )),
        must_ok(UpstreamEndpoint::new("http://127.0.0.1:1/v1")),
        state_path.clone(),
        secret_root.clone(),
    );
    let runtime = must_ok(LoopbackRouterRuntime::start(runtime_config, secrets.clone()).await);
    let refresh_tasks = runtime.credential_refresh_task_supervisor();
    assert!(matches!(runtime.serve_protocol_connections(0).await, Ok(0)));
    drop(runtime);

    let (refresh_client, mut refresh_entries, refresh_release) = held_producer_refresh_client(
        account_id.clone(),
        "closed-quota-401-account-original-refresh",
    );
    let refresh_calls = Arc::clone(&refresh_client.calls);
    let resolver = must_ok(
        AsyncCliCredentialResolver::open_with_refresh_client(
            &state_path,
            secrets.clone(),
            refresh_client,
            refresh_tasks,
        )
        .await,
    );
    let (quota_attempt_sender, mut quota_attempt_receiver) = tokio::sync::mpsc::unbounded_channel();
    let quota_provider_calls = Arc::new(AtomicUsize::new(0));
    let observed_quota_provider_calls = Arc::clone(&quota_provider_calls);
    let (quota_cycle_sender, mut quota_cycle_receiver) = tokio::sync::mpsc::unbounded_channel();
    let quota_worker = start_background_quota_refresh_worker_with_clock(
        state_path.clone(),
        secret_root,
        "https://chatgpt.com/backend-api".to_owned(),
        resolver,
        UnauthorizedQuotaRefreshProvider {
            calls: observed_quota_provider_calls,
            entered_sender: quota_attempt_sender,
        },
        move || {
            quota_cycle_sender
                .send(())
                .expect("quota scheduler should report its 401 cycle");
            1_000
        },
        Duration::ZERO,
    )
    .await;
    tokio::time::timeout(Duration::from_secs(3), quota_cycle_receiver.recv())
        .await
        .expect("quota scheduler should start its cycle")
        .expect("quota scheduler should report its cycle");
    tokio::time::timeout(Duration::from_secs(3), quota_attempt_receiver.recv())
        .await
        .expect("quota should make one 401 provider request")
        .expect("401 request should be observed");

    let _ = refresh_release.send(());
    let mut quota_worker = quota_worker;
    quota_worker.shutdown().await;
    assert_eq!(quota_provider_calls.load(Ordering::SeqCst), 1);
    assert_eq!(refresh_calls.load(Ordering::SeqCst), 0);
    assert!(refresh_entries.try_recv().is_err());

    let state = must_ok(SqliteStateStore::open(&state_path));
    let account = must_ok(AccountStateRepository::load_account(&state, &account_id))
        .expect("quota account should remain stored");
    assert_eq!(account.active_credential_generation(), Some(1));
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    let active_bundle = must_ok(secrets.read_secret(&active_key));
    let active_bundle = must_ok(AccountCredentialBundle::from_secret_string(active_bundle));
    assert_eq!(
        active_bundle
            .refresh_token()
            .expect("active credential should remain renewable")
            .expose_secret(),
        "closed-quota-401-account-original-refresh"
    );
}
