#![allow(unused_imports)]
use super::*;

#[test]
pub(super) fn runtime_affinity_cache_uses_configured_session_pin_idle_ttl() {
    let configured_ttl = Duration::from_secs(30 * 60);
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("loopback bind"),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1").expect("fixture upstream"),
        PathBuf::from("unused-state.sqlite"),
        PathBuf::from("unused-secrets"),
    )
    .with_session_pin_idle_ttl(configured_ttl);
    let runtime_cache = config.session_account_affinity_cache();
    let account_id = AccountId::new("runtime-pin-account").expect("account id");

    let selection = crate::session_account_affinity_cache::publish_session_account_affinity(
        &runtime_cache,
        codex_router_core::provider::Provider::Openai,
        "runtime-pin-session",
        &account_id,
        RouteBand::Responses,
        None,
        1_000,
    )
    .expect("runtime cache should publish the pin");
    drop(selection);

    for (now_unix_seconds, expected_fresh) in [(2_799, true), (2_800, false)] {
        let lookup = crate::session_account_affinity_cache::lookup_session_account_affinity(
            &runtime_cache,
            codex_router_core::provider::Provider::Openai,
            "runtime-pin-session",
            RouteBand::Responses,
            None,
            now_unix_seconds,
        )
        .expect("runtime cache lookup should succeed");
        assert_eq!(lookup.is_some(), expected_fresh, "time={now_unix_seconds}");
    }
}

pub(super) fn open_read_only_after_shutdown(
    runtime: &tokio::runtime::Runtime,
    database_path: &Path,
    case: &str,
) -> AsyncSqliteStateStore {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match runtime.block_on(AsyncSqliteStateStore::open_read_only(database_path)) {
            Ok(state) => return state,
            Err(error)
                if format!("{error}").contains("locked")
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now()
            }
            Err(error) => panic!("{case} read-only state unavailable: {error}"),
        }
    }
}

#[test]
pub(super) fn proxy_shutdown_drains_claimed_refresh_or_preserves_unresolved_claim_at_bound() {
    for (case, drain_limit, complete_before_shutdown) in [
        ("completed", Duration::from_secs(2), true),
        ("bounded", Duration::from_millis(100), false),
    ] {
        let (router, account_id, database_path, _secrets) =
            proxy_refresh_fixture(case, drain_limit);
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let (completed_sender, completed_receiver) = mpsc::channel();
        let client = HeldProxyRefreshClient {
            entered_sender,
            release_receiver: Arc::new(Mutex::new(release_receiver)),
            completed_sender,
        };
        let resolver = router
            .credential_factory
            .resolver_for_state_with_refresh_client(router.credential_state_store.clone(), client);
        let account_for_task = account_id.clone();
        let request_task = router
            .runtime
            .as_ref()
            .expect("runtime")
            .handle()
            .spawn(async move {
                resolver
                    .resolve_provider_credentials(
                        &account_for_task,
                        codex_router_core::provider::Provider::Openai,
                    )
                    .await
            });
        entered_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("provider should start");
        request_task.abort();
        let shutdown = CancellationToken::new();
        shutdown.cancel();
        let (stopped_sender, stopped_receiver) = mpsc::channel();
        let shutdown_thread = std::thread::spawn(move || {
            let result = router.serve_protocol_connections_until_cancelled(usize::MAX, shutdown);
            stopped_sender.send(result).expect("shutdown should report");
        });
        if complete_before_shutdown {
            assert!(matches!(
                stopped_receiver.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            release_sender.send(()).expect("release provider");
        }
        let result = stopped_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("proxy shutdown should finish")
            .expect("proxy shutdown should succeed");
        assert_eq!(result, 0);
        shutdown_thread.join().expect("shutdown thread");
        if !complete_before_shutdown {
            release_sender
                .send(())
                .expect("release provider after bound");
        }
        completed_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("provider should finish");
        let read_runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("read runtime");
        let read_state = open_read_only_after_shutdown(&read_runtime, &database_path, case);
        let account = read_runtime
            .block_on(read_state.load_account(&account_id))
            .expect("account read")
            .expect("account exists");
        let maintenance = read_runtime
            .block_on(read_state.load_credential_maintenance(&account_id))
            .expect("maintenance read")
            .expect("claim exists");
        if complete_before_shutdown {
            assert_eq!(account.active_credential_generation(), Some(2));
            assert_eq!(maintenance.state, CredentialMaintenanceState::Healthy);
        } else {
            assert_eq!(account.active_credential_generation(), Some(1));
            assert_eq!(maintenance.state, CredentialMaintenanceState::InProgress);
            assert_eq!(maintenance.claimed_successor_generation, Some(2));
        }
        read_runtime
            .block_on(read_state.close())
            .expect("read state close");
    }
}

#[derive(Clone)]
pub(super) struct ImmediateProxyRefreshClient;

impl CredentialRefreshClient for ImmediateProxyRefreshClient {
    fn refresh_credentials(
        &self,
        _account_id: &AccountId,
        _refresh_token: &SecretString,
    ) -> Result<AccountCredentialBundle, CredentialRefreshFailure> {
        Ok(AccountCredentialBundle::imported_codex_auth(
            "replacement-access-canary",
            Some("replacement-refresh-canary".to_owned()),
        )
        .with_expires_unix_seconds(2_000))
    }
}

#[derive(Clone)]
pub(super) struct HeldProxySecretWriteStore {
    pub(super) inner: EncryptedCredentialStore,
    pub(super) entered_sender: mpsc::Sender<()>,
    pub(super) release_receiver: Arc<Mutex<mpsc::Receiver<()>>>,
}

impl SecretStore for HeldProxySecretWriteStore {
    fn write_secret(&self, key: &SecretKey, secret: &SecretString) -> Result<(), SecretStoreError> {
        if key.as_str().ends_with(".2") {
            self.entered_sender
                .send(())
                .expect("secret write should report");
            self.release_receiver
                .lock()
                .expect("write release lock")
                .recv_timeout(Duration::from_secs(5))
                .expect("secret write should be released");
        }
        self.inner.write_secret(key, secret)
    }

    fn delete_staged(&self, key: &SecretKey) -> Result<(), SecretStoreError> {
        self.inner.delete_staged(key)
    }

    fn read_secret(&self, key: &SecretKey) -> Result<SecretString, SecretStoreError> {
        self.inner.read_secret(key)
    }
}

#[test]
pub(super) fn proxy_shutdown_waits_for_blocking_successor_write_before_returning() {
    let (router, account_id, database_path, file_secrets) =
        proxy_refresh_fixture("held-successor-write", Duration::from_secs(2));
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let secrets = HeldProxySecretWriteStore {
        inner: file_secrets,
        entered_sender,
        release_receiver: Arc::new(Mutex::new(release_receiver)),
    };
    let resolver = router
        .credential_factory
        .resolver_for_state_with_dependencies(
            router.credential_state_store.clone(),
            secrets,
            ImmediateProxyRefreshClient,
        );
    let account_for_task = account_id.clone();
    let request_task = router
        .runtime
        .as_ref()
        .expect("runtime")
        .handle()
        .spawn(async move {
            resolver
                .resolve_provider_credentials(
                    &account_for_task,
                    codex_router_core::provider::Provider::Openai,
                )
                .await
        });
    entered_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("successor write should start");
    request_task.abort();
    let shutdown = CancellationToken::new();
    shutdown.cancel();
    let (stopped_sender, stopped_receiver) = mpsc::channel();
    let shutdown_thread = std::thread::spawn(move || {
        let result = router.serve_protocol_connections_until_cancelled(usize::MAX, shutdown);
        stopped_sender.send(result).expect("shutdown should report");
    });
    assert!(matches!(
        stopped_receiver.recv_timeout(Duration::from_millis(100)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release_sender.send(()).expect("release successor write");
    assert_eq!(
        stopped_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("shutdown should finish")
            .expect("shutdown should succeed"),
        0,
    );
    shutdown_thread.join().expect("shutdown thread");
    let read_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("read runtime");
    let read_state =
        open_read_only_after_shutdown(&read_runtime, &database_path, "held successor write");
    let account = read_runtime
        .block_on(read_state.load_account(&account_id))
        .expect("account read")
        .expect("account exists");
    assert_eq!(account.active_credential_generation(), Some(2));
    read_runtime
        .block_on(read_state.close())
        .expect("read state close");
}

#[test]
pub(super) fn active_session_event_compaction_keeps_completed_events_for_seven_days() {
    assert_eq!(
        active_session_event_compaction_before(7 * 86_400 + 123),
        123
    );
    assert_eq!(active_session_event_compaction_before(7 * 86_400), 0);
    assert_eq!(active_session_event_compaction_before(123), 0);
}

#[test]
pub(super) fn daily_session_affinity_cleanup_guard_allows_only_advancing_utc_days() {
    let last_attempted_utc_day = std::sync::atomic::AtomicU64::new(u64::MAX);

    assert!(claim_session_affinity_cleanup_day(
        &last_attempted_utc_day,
        0,
    ));
    assert!(!claim_session_affinity_cleanup_day(
        &last_attempted_utc_day,
        86_399,
    ));
    assert!(claim_session_affinity_cleanup_day(
        &last_attempted_utc_day,
        86_400,
    ));
    assert!(!claim_session_affinity_cleanup_day(
        &last_attempted_utc_day,
        1,
    ));
}

#[tokio::test]
pub(super) async fn compatibility_health_response_is_static_and_bypasses_model_authentication() {
    let uri = Uri::from_static("/healthz");
    let response = router_compatibility_response(&HttpMethod::GET, &uri, true)
        .unwrap_or_else(|| panic!("GET /healthz should return a compatibility response"));

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .unwrap_or_else(|error| panic!("compatibility body should collect: {error}"))
        .to_bytes();
    let payload = serde_json::from_slice::<
        codex_router_core::router_compatibility::RouterCompatibility,
    >(&body)
    .unwrap_or_else(|error| panic!("compatibility body should decode: {error}"));

    assert_eq!(
        payload,
        codex_router_core::router_compatibility::RouterCompatibility::current(true)
    );
}

#[test]
pub(super) fn compatibility_health_response_rejects_other_methods_and_paths() {
    assert!(
        router_compatibility_response(&HttpMethod::POST, &Uri::from_static("/healthz"), false,)
            .is_none()
    );
    assert!(
        router_compatibility_response(&HttpMethod::GET, &Uri::from_static("/v1/models"), false,)
            .is_none()
    );
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordingAsyncAffinityOwnerRecorder {
    pub(super) records: Arc<Mutex<Vec<PreviousResponseAffinityOwnerRecord>>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct RecordedHttpProviderError {
    pub(super) account_id: codex_router_core::ids::AccountId,
    pub(super) route_band: RouteBand,
    pub(super) classification: ProviderErrorClassification,
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordingAsyncProviderErrorObserver {
    pub(super) records: Arc<Mutex<Vec<RecordedHttpProviderError>>>,
}

impl RecordingAsyncProviderErrorObserver {
    pub(super) fn records(&self) -> Vec<RecordedHttpProviderError> {
        match self.records.lock() {
            Ok(records) => records.clone(),
            Err(error) => panic!("test provider observer lock should be available: {error}"),
        }
    }
}

impl AsyncProviderErrorObserver for RecordingAsyncProviderErrorObserver {
    fn observe_provider_error<'a>(
        &'a self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            match self.records.lock() {
                Ok(mut records) => records.push(RecordedHttpProviderError {
                    account_id,
                    route_band,
                    classification,
                }),
                Err(error) => {
                    panic!("test provider observer lock should be available: {error}")
                }
            }
            Ok(())
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        match self.records.lock() {
            Ok(mut records) => records.push(RecordedHttpProviderError {
                account_id,
                route_band,
                classification,
            }),
            Err(error) => panic!("test provider observer lock should be available: {error}"),
        }
        DbWriteEnqueueResult::Enqueued
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct NonblockingHttpQuotaObserver {
    pub(super) durable_observation_called: Arc<AtomicBool>,
    pub(super) enqueued_records: Arc<Mutex<Vec<RecordedHttpProviderError>>>,
}

impl NonblockingHttpQuotaObserver {
    pub(super) fn enqueued_records(&self) -> Vec<RecordedHttpProviderError> {
        lock_test_mutex(&self.enqueued_records, "http quota enqueue records").clone()
    }
}

impl AsyncProviderErrorObserver for NonblockingHttpQuotaObserver {
    fn mark_runtime_account_quota_exhausted(
        &self,
        _account_id: codex_router_core::ids::AccountId,
        _route_band: RouteBand,
        _observed_unix_seconds: u64,
    ) -> Result<(), ProviderErrorObservationError> {
        Ok(())
    }

    fn observe_provider_error<'a>(
        &'a self,
        _account_id: codex_router_core::ids::AccountId,
        _route_band: RouteBand,
        _classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> BoxFuture<'a, Result<(), ProviderErrorObservationError>> {
        Box::pin(async move {
            self.durable_observation_called
                .store(true, Ordering::SeqCst);
            std::future::pending().await
        })
    }

    fn enqueue_provider_quota_exhaustion(
        &self,
        account_id: codex_router_core::ids::AccountId,
        route_band: RouteBand,
        classification: ProviderErrorClassification,
        _observed_unix_seconds: u64,
    ) -> DbWriteEnqueueResult {
        lock_test_mutex(&self.enqueued_records, "http quota enqueue records").push(
            RecordedHttpProviderError {
                account_id,
                route_band,
                classification,
            },
        );
        DbWriteEnqueueResult::Enqueued
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordingLoopbackConnectionErrorReporter {
    pub(super) diagnostics: Arc<Mutex<Vec<String>>>,
}

impl RecordingLoopbackConnectionErrorReporter {
    pub(super) fn diagnostics(&self) -> Vec<String> {
        lock_test_mutex(&self.diagnostics, "connection diagnostics").clone()
    }
}

impl LoopbackConnectionErrorReporter for RecordingLoopbackConnectionErrorReporter {
    fn report_connection_error(&self, diagnostic: &str) {
        lock_test_mutex(&self.diagnostics, "connection diagnostics").push(diagnostic.to_owned());
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct RecordingActiveClientLeaseReporter {
    pub(super) released: Arc<Mutex<Vec<(String, String)>>>,
}

impl RecordingActiveClientLeaseReporter {
    pub(super) fn released(&self) -> Vec<(String, String)> {
        lock_test_mutex(&self.released, "active lease releases").clone()
    }
}

impl crate::account_selection::ActiveClientLeaseReporter for RecordingActiveClientLeaseReporter {
    fn record_acquired(
        &self,
        _route_band: &str,
        _reservation_handle: &codex_router_selection::reservation::ReservationHandle,
        _acquired_unix_seconds: u64,
        _active_pressure: u32,
    ) {
    }

    fn record_released(
        &self,
        route_band: &str,
        reservation_handle: &codex_router_selection::reservation::ReservationHandle,
    ) {
        lock_test_mutex(&self.released, "active lease releases").push((
            route_band.to_owned(),
            reservation_handle.reservation_id().as_str().to_owned(),
        ));
    }
}
