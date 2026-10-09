use super::*;
use crate::resolver::CredentialRefreshFailure;
use crate::resolver::CredentialRefreshTaskSupervisor;
use codex_router_core::provider::Provider;

#[derive(Clone, Copy)]
enum RenewalRequestKind {
    Expired,
    Proactive,
    Unauthorized,
}

#[tokio::test]
async fn closed_supervisor_rejects_expired_credential_renewal_before_provider_egress() {
    let temp_dir = AuthTestTempDir::new("closed-refresh-admission");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("closed-refresh-account");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    "closed refresh",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(openai_account_credential_bundle_key(&account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "expired-access-canary",
                    Some("refresh-token-canary".to_owned()),
                )
                .with_expires_unix_seconds(900)
                .to_secret_string(),
            ),
        ),
    );
    let refresh_client = RecordingRefreshClient::new_for_account(
        account_id.as_str(),
        "refresh-token-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    );
    let refresh_tasks = CredentialRefreshTaskSupervisor::new();
    assert!(refresh_tasks.drain(Duration::from_secs(1)).await);
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets,
        refresh_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(refresh_tasks);

    let result = resolver
        .resolve_provider_credentials(&account_id, Provider::Openai)
        .await;
    let active_generation = must_ok(state.load_account(&account_id).await)
        .expect("account remains stored")
        .active_credential_generation();

    assert_eq!(result, Err(CredentialResolverError::RenewalAdmissionClosed));
    assert_eq!(refresh_client.calls(), 0);
    assert_eq!(active_generation, Some(1));
}

#[tokio::test]
async fn closed_admission_refuses_all_renewal_triggers_and_leaves_login_independent() {
    let temp_dir = AuthTestTempDir::new("closed-all-renewal-triggers");
    let state = must_ok(AsyncSqliteStateStore::open(&temp_dir.path().join("state.sqlite")).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let refresh_tasks = CredentialRefreshTaskSupervisor::new();
    let shared_clone = refresh_tasks.clone();
    refresh_tasks.close_admission();
    let closed_error = CredentialResolverError::RenewalAdmissionClosed;
    assert_eq!(closed_error.clone(), closed_error);
    assert_eq!(
        closed_error.to_string(),
        "provider credential renewal admission is closed"
    );
    let cases = [
        ("expired", RenewalRequestKind::Expired, 900),
        ("proactive", RenewalRequestKind::Proactive, 2_000),
        ("unauthorized", RenewalRequestKind::Unauthorized, 2_000),
    ];

    for (name, request_kind, expiry) in cases {
        let account_id = account_id(&format!("closed-{name}-account"));
        seed_openai_account(
            &state,
            &secrets,
            &account_id,
            expiry,
            "refresh-token-canary",
        )
        .await;
        let account_before =
            must_ok(state.load_account(&account_id).await).expect("seeded account remains stored");
        let maintenance_before = must_ok(state.load_credential_maintenance(&account_id).await);
        let refresh_client = recording_refresh_client(&account_id);
        let resolver = AsyncRouterCredentialResolver::new(
            state.clone(),
            secrets.clone(),
            refresh_client.clone(),
            Some(1_000),
        )
        .with_refresh_task_supervisor(shared_clone.clone());

        let result = match request_kind {
            RenewalRequestKind::Expired => resolver
                .resolve_provider_credentials(&account_id, Provider::Openai)
                .await
                .map(|_| ()),
            RenewalRequestKind::Proactive => {
                resolver.maintain_account_credentials(&account_id).await
            }
            RenewalRequestKind::Unauthorized => resolver
                .recover_unauthorized_credentials(&account_id, Provider::Openai, 1)
                .await
                .map(|_| ()),
        };

        assert_eq!(
            result,
            Err(CredentialResolverError::RenewalAdmissionClosed),
            "{name} trigger"
        );
        assert_eq!(refresh_client.calls(), 0, "{name} provider calls");
        assert_eq!(
            must_ok(state.load_account(&account_id).await),
            Some(account_before),
            "{name} account state"
        );
        assert_eq!(
            must_ok(state.load_credential_maintenance(&account_id).await),
            maintenance_before,
            "{name} maintenance state"
        );
        let successor_key = must_ok(openai_account_credential_bundle_key(&account_id, 2));
        assert!(
            secrets.read_secret(&successor_key).is_err(),
            "{name} successor secret must remain absent"
        );
    }

    let valid_account = account_id("valid-credential-after-close");
    seed_openai_account(
        &state,
        &secrets,
        &valid_account,
        2_000,
        "refresh-token-canary",
    )
    .await;
    let valid_client = recording_refresh_client(&valid_account);
    let valid_resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        valid_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(shared_clone.clone());
    assert_eq!(
        must_ok(
            valid_resolver
                .resolve_provider_credentials(&valid_account, Provider::Openai)
                .await
        )
        .credential_generation(),
        1,
        "valid credentials remain available after close"
    );
    assert_eq!(valid_client.calls(), 0);

    let independent_account = account_id("independent-open-supervisor");
    seed_openai_account(
        &state,
        &secrets,
        &independent_account,
        900,
        "refresh-token-canary",
    )
    .await;
    let independent_client = recording_refresh_client(&independent_account);
    let independent_resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        independent_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(CredentialRefreshTaskSupervisor::new());
    let independent_result = independent_resolver
        .resolve_provider_credentials(&independent_account, Provider::Openai)
        .await;
    assert_eq!(
        must_ok(independent_result).credential_generation(),
        2,
        "a separately constructed supervisor stays open"
    );
    assert_eq!(independent_client.calls(), 1);

    let login_account = account_id("login-after-renewal-close");
    seed_openai_account(
        &state,
        &secrets,
        &login_account,
        2_000,
        "refresh-token-canary",
    )
    .await;
    let login_generation = must_ok(
        crate::credential_activation::CredentialActivation::activate_login(
            &state,
            &secrets,
            crate::credential_activation::CredentialActivationRequest::new(
                Provider::Openai,
                login_account.clone(),
                "login after proxy close",
                AccountCredentialBundle::imported_codex_auth(
                    "login-access-canary",
                    Some("login-refresh-canary".to_owned()),
                )
                .into(),
            ),
        )
        .await,
    );
    assert_eq!(login_generation, 2);
    assert_eq!(
        must_ok(state.load_account(&login_account).await)
            .expect("login account remains stored")
            .active_credential_generation(),
        Some(2)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_cancels_in_process_lock_waiter_but_finishes_claimed_rotation_after_caller_cancel() {
    let temp_dir = AuthTestTempDir::new("close-in-process-lock-waiter");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("close-in-process-lock-waiter");
    seed_openai_account(&state, &secrets, &account_id, 900, "refresh-token-canary").await;
    let refresh_tasks = CredentialRefreshTaskSupervisor::new();
    let (task_pending_sender, mut task_pending_receiver) = tokio::sync::mpsc::unbounded_channel();
    refresh_tasks.set_test_task_pending_observer(task_pending_sender);
    let (refresh_client, provider_entered_receiver, provider_release_sender) =
        blocking_refresh_client(2_000);
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(refresh_tasks.clone());

    let first_account = account_id.clone();
    let first_waiter = tokio::spawn({
        let resolver = resolver.clone();
        async move {
            resolver
                .resolve_provider_credentials(&first_account, Provider::Openai)
                .await
        }
    });
    assert!(task_pending_receiver.recv().await.is_some());
    must_ok(
        tokio::task::spawn_blocking(move || {
            provider_entered_receiver.recv_timeout(Duration::from_secs(2))
        })
        .await,
    )
    .expect("first rotation reaches the synthetic provider");

    let second_account = account_id.clone();
    let second_waiter = tokio::spawn(async move {
        resolver
            .resolve_provider_credentials(&second_account, Provider::Openai)
            .await
    });
    assert!(
        task_pending_receiver.recv().await.is_some(),
        "second tracked task is pending while the first owns the in-process lease"
    );
    refresh_tasks.close_admission();
    first_waiter.abort();
    assert!(first_waiter.await.is_err(), "request caller was canceled");
    assert_eq!(
        must_ok(tokio::time::timeout(Duration::from_secs(2), second_waiter).await)
            .expect("queued request returns after close"),
        Err(CredentialResolverError::RenewalAdmissionClosed)
    );
    assert!(
        !refresh_tasks.drain(Duration::from_millis(10)).await,
        "drain timeout must leave the claimed provider task tracked"
    );
    let in_progress = must_ok(state.load_credential_maintenance(&account_id).await)
        .expect("first task owns its refresh claim");
    assert_eq!(in_progress.state, CredentialMaintenanceState::InProgress);
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        must_ok(state.load_account(&account_id).await)
            .expect("account remains stored")
            .active_credential_generation(),
        Some(1)
    );

    must_ok(provider_release_sender.send(()));
    assert!(refresh_tasks.drain(Duration::from_secs(2)).await);
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        must_ok(state.load_account(&account_id).await)
            .expect("account remains stored")
            .active_credential_generation(),
        Some(2)
    );
    let successor_key = must_ok(openai_account_credential_bundle_key(&account_id, 2));
    assert!(secrets.read_secret(&successor_key).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_admission_and_close_tracks_every_accepted_rotation() {
    const REQUEST_COUNT: usize = 12;

    let temp_dir = AuthTestTempDir::new("renewal-admission-close-race");
    let database_path = temp_dir.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&database_path).await);
    let secrets = must_ok(
        codex_router_secret_store::test_support::open_encrypted_credential_store(
            temp_dir.path().join("secrets"),
        ),
    );
    let account_id = account_id("renewal-admission-close-race");
    seed_openai_account(&state, &secrets, &account_id, 900, "refresh-token-canary").await;
    let refresh_tasks = CredentialRefreshTaskSupervisor::new();
    let (refresh_client, provider_entered_receiver, provider_release_sender) =
        blocking_refresh_client(2_000);
    let resolver = AsyncRouterCredentialResolver::new(
        state.clone(),
        secrets.clone(),
        refresh_client.clone(),
        Some(1_000),
    )
    .with_refresh_task_supervisor(refresh_tasks.clone());
    let start_gate = Arc::new(tokio::sync::Barrier::new(REQUEST_COUNT + 1));
    let mut request_tasks = Vec::with_capacity(REQUEST_COUNT);

    for _ in 0..REQUEST_COUNT {
        let resolver = resolver.clone();
        let account_id = account_id.clone();
        let start_gate = Arc::clone(&start_gate);
        request_tasks.push(tokio::spawn(async move {
            start_gate.wait().await;
            resolver
                .resolve_provider_credentials(&account_id, Provider::Openai)
                .await
        }));
    }
    start_gate.wait().await;
    must_ok(
        tokio::task::spawn_blocking(move || {
            provider_entered_receiver.recv_timeout(Duration::from_secs(3))
        })
        .await,
    )
    .expect("one accepted renewal reaches the provider barrier");

    refresh_tasks.close_admission();
    assert_eq!(
        resolver
            .resolve_provider_credentials(&account_id, Provider::Openai)
            .await,
        Err(CredentialResolverError::RenewalAdmissionClosed),
        "no renewal can spawn after close"
    );
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    assert!(
        !refresh_tasks
            .wait_for_completion(Duration::from_millis(10))
            .await,
        "the accepted provider task remains part of the drain"
    );

    must_ok(provider_release_sender.send(()));
    assert!(refresh_tasks.drain(Duration::from_secs(3)).await);
    assert_eq!(refresh_client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        must_ok(state.load_account(&account_id).await)
            .expect("account remains stored")
            .active_credential_generation(),
        Some(2)
    );
    let successor_key = must_ok(openai_account_credential_bundle_key(&account_id, 2));
    assert!(secrets.read_secret(&successor_key).is_ok());

    for request_task in request_tasks {
        let result = must_ok(tokio::time::timeout(Duration::from_secs(3), request_task).await)
            .expect("every concurrent request is accounted for");
        match result {
            Ok(credential) => assert_eq!(credential.credential_generation(), 2),
            Err(CredentialResolverError::RenewalAdmissionClosed) => {}
            Err(error) => panic!("unexpected concurrent renewal outcome: {error}"),
        }
    }
}

pub(super) fn recording_refresh_client(account_id: &AccountId) -> RecordingRefreshClient {
    RecordingRefreshClient::new_for_account(
        account_id.as_str(),
        "refresh-token-canary",
        AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000),
    )
}

pub(super) async fn seed_openai_account<S: SecretStore>(
    state: &AsyncSqliteStateStore,
    secrets: &S,
    account_id: &AccountId,
    expires_unix_seconds: u64,
    refresh_token: &str,
) {
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Openai,
                    account_id.clone(),
                    "supervision test account",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let active_key = must_ok(openai_account_credential_bundle_key(account_id, 1));
    must_ok(
        secrets.write_secret(
            &active_key,
            &must_ok(
                AccountCredentialBundle::imported_codex_auth(
                    "active-access-canary",
                    Some(refresh_token.to_owned()),
                )
                .with_expires_unix_seconds(expires_unix_seconds)
                .to_secret_string(),
            ),
        ),
    );
}

fn blocking_refresh_client(
    response_expiry_unix_seconds: u64,
) -> (
    BlockingSyntheticRefreshClient,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered_sender, entered_receiver) = std::sync::mpsc::channel();
    let (release_sender, release_receiver) = std::sync::mpsc::channel();
    (
        BlockingSyntheticRefreshClient {
            calls: Arc::new(AtomicUsize::new(0)),
            entered_sender,
            release_receiver: Arc::new(Mutex::new(release_receiver)),
            response_expiry_unix_seconds,
        },
        entered_receiver,
        release_sender,
    )
}

#[derive(Clone)]
struct BlockingSyntheticRefreshClient {
    calls: Arc<AtomicUsize>,
    entered_sender: std::sync::mpsc::Sender<()>,
    release_receiver: Arc<Mutex<std::sync::mpsc::Receiver<()>>>,
    response_expiry_unix_seconds: u64,
}

impl CredentialRefreshClient for BlockingSyntheticRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered_sender
            .send(())
            .expect("provider entry should be observed");
        self.release_receiver
            .lock()
            .expect("provider release lock")
            .recv_timeout(Duration::from_secs(5))
            .expect("provider barrier should be released");
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(self.response_expiry_unix_seconds))
    }
}
