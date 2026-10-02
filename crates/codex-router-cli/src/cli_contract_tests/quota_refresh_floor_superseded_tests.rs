use super::*;
use crate::quota::QuotaRefreshSchedule;
use codex_router_core::credit_usage::CreditProviderObservation;
use codex_router_state::credit_store::ResponsesRefreshSuccessCommit;
use codex_router_state::quota_snapshot::QuotaHistoryRefreshOutcome;

fn persisted_responses_window(
    account_id: &AccountId,
    limit_window_seconds: u64,
    remaining_percent: u32,
    observed_unix_seconds: u64,
) -> PersistedSelectorQuotaWindow {
    let status = if remaining_percent == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    PersistedSelectorQuotaWindow::new(
        account_id.clone(),
        "responses",
        limit_window_seconds,
        status,
    )
    .with_remaining_headroom(remaining_percent)
    .with_effective(true)
    .with_observed_unix_seconds(observed_unix_seconds)
    .with_reset_unix_seconds(observed_unix_seconds + limit_window_seconds)
}

fn successful_responses_history_observation(
    account_id: &AccountId,
    limit_window_seconds: u64,
    remaining_percent: u32,
    observed_unix_seconds: u64,
) -> PersistedQuotaHistoryObservation {
    let status = if remaining_percent == 0 {
        SelectorQuotaWindowStatus::Ineligible
    } else {
        SelectorQuotaWindowStatus::Eligible
    };
    PersistedQuotaHistoryObservation::new(
        account_id.clone(),
        account_id.as_str(),
        "responses",
        limit_window_seconds,
        observed_unix_seconds,
        remaining_percent,
    )
    .with_reset_unix_seconds(observed_unix_seconds + limit_window_seconds)
    .with_effective(true)
    .with_window_status(status)
    .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
    .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
}

fn successful_responses_snapshot(
    account_id: &AccountId,
    remaining_percent: u32,
    observed_unix_seconds: u64,
) -> PersistedQuotaSnapshot {
    PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_route_band("responses", remaining_percent)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800)
        .with_stale_penalty(false)
}

fn provider_response_with_weekly_percent(
    weekly_remaining_percent: u32,
) -> QuotaRefreshProviderResponse {
    QuotaRefreshProviderResponse {
        windows: vec![
            QuotaRefreshProviderWindow {
                limit_window_seconds: 18_000,
                headroom: crate::quota::QuotaWindowHeadroom::Percent(80),
                reset_unix_seconds: Some(18_000),
                effective: true,
            },
            QuotaRefreshProviderWindow {
                limit_window_seconds: 604_800,
                headroom: crate::quota::QuotaWindowHeadroom::Percent(weekly_remaining_percent),
                reset_unix_seconds: Some(604_800),
                effective: true,
            },
        ],
        reset_credits_available: None,
        credit_provider_observation: CreditProviderObservation::missing(),
    }
}

async fn read_floor_http_request(stream: &mut tokio::net::TcpStream) -> String {
    use tokio::io::AsyncReadExt;

    let mut request_bytes = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        let bytes_read = stream
            .read(&mut byte)
            .await
            .unwrap_or_else(|error| panic!("loopback quota provider should read: {error}"));
        if bytes_read == 0 {
            break;
        }
        request_bytes.push(byte[0]);
        if request_bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    String::from_utf8_lossy(&request_bytes).into_owned()
}

async fn write_floor_http_response(stream: &mut tokio::net::TcpStream, body: &str) {
    use tokio::io::AsyncWriteExt;

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .unwrap_or_else(|error| panic!("loopback quota provider should respond: {error}"));
}

fn expired_zero_weekly_usage_body() -> &'static str {
    r#"{
        "rate_limit": {
            "primary_window": {
                "used_percent": 20,
                "reset_at": 18000,
                "limit_window_seconds": 18000
            },
            "secondary_window": {
                "used_percent": 100,
                "reset_at": 604800,
                "limit_window_seconds": 604800
            }
        },
        "additional_rate_limits": []
    }"#
}

struct SupersededResponsesQuotaProvider {
    response_started: Arc<tokio::sync::Notify>,
    release_response: Arc<tokio::sync::Notify>,
}

struct FloorSignalCredentialResolver;

impl AsyncProviderCredentialResolver for FloorSignalCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        _expected_provider: codex_router_core::provider::Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("generation-one-token"),
            1,
        ))
    }
}

impl QuotaRefreshProvider for SupersededResponsesQuotaProvider {
    async fn fetch_quota(
        &self,
        request: QuotaRefreshProviderRequest,
    ) -> Result<QuotaRefreshProviderResponse, crate::quota::QuotaCommandError> {
        if request.route_band() == "responses" {
            self.response_started.notify_one();
            self.release_response.notified().await;
            Ok(provider_response_with_weekly_percent(0))
        } else {
            Ok(provider_response_with_weekly_percent(80))
        }
    }
}

#[tokio::test]
async fn superseded_responses_completion_notifies_from_latest_committed_weekly_window() {
    let test_root = TestRoot::new("quota-refresh-superseded-floor-intent");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_superseded_floor_intent");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "superseded-floor-intent",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let floor = WeeklyQuotaFloorBasisPoints::new(500).expect("5 percent floor should be valid");
    let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
    must_ok(
        mutation
            .set_weekly_quota_floor_by_account_id(&account_id, Some(floor))
            .await,
    );
    mutation.close().await;

    let response_started = Arc::new(tokio::sync::Notify::new());
    let release_response = Arc::new(tokio::sync::Notify::new());
    let provider = SupersededResponsesQuotaProvider {
        response_started: Arc::clone(&response_started),
        release_response: Arc::clone(&release_response),
    };
    let resolver = FloorSignalCredentialResolver;
    let observer = Arc::new(RecordingWeeklyFloorObserver::default());
    let refresh_observer = Arc::clone(&observer);
    let secret_root = test_root.path().join("unused-secrets");
    let refresh_state_path = state_path.clone();
    let mut refresh = tokio::spawn(async move {
        let mut output = Vec::new();
        let result = refresh_quota_store_paths_with_dependencies_and_floor_notifier_async(
            &mut output,
            &refresh_state_path,
            &secret_root,
            "http://unused-provider".to_owned(),
            &resolver,
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds: 1_100,
                schedule: QuotaRefreshSchedule::Background {
                    interval_seconds: 180,
                },
                weekly_floor_observer: Some(refresh_observer.as_ref()),
            },
        )
        .await;
        (result, output)
    });

    let response_started = response_started.notified();
    tokio::pin!(response_started);
    tokio::select! {
        () = &mut response_started => {}
        result = &mut refresh => panic!(
            "background refresh should pause at the older Responses provider read: {result:?}"
        ),
    }

    let latest_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("newer same-generation refresh should allocate");
    let latest_windows = [
        persisted_responses_window(&account_id, 18_000, 80, 1_100),
        persisted_responses_window(&account_id, 604_800, 6, 1_100),
    ];
    let latest_history = [
        successful_responses_history_observation(&account_id, 18_000, 80, 1_100),
        successful_responses_history_observation(&account_id, 604_800, 6, 1_100),
    ];
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &latest_attempt,
            selector_windows: &latest_windows,
            observed_unix_seconds: 1_100,
            stale_after_unix_seconds: 1_460,
            provider_observation: &CreditProviderObservation::missing(),
            history_observations: &latest_history,
            snapshot: &successful_responses_snapshot(&account_id, 6, 1_100),
        })
        .await
        .expect("newer successful observation should commit before the older read returns");
    release_response.notify_one();

    let (refresh_result, _output) = refresh
        .await
        .unwrap_or_else(|error| panic!("background refresh should complete: {error}"));
    must_ok(refresh_result);
    assert_eq!(
        *lock_test_mutex(&observer.account_ids, "weekly floor accounts"),
        vec![account_id.clone()]
    );
    assert_eq!(
        *lock_test_mutex(&observer.intents, "weekly floor intents"),
        vec![WeeklyQuotaFloorIntent::GracefulSwitch],
        "the rejected older zero-window response must not signal HardStop or drop the latest committed floor intent"
    );

    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("latest credit observation should load")
        .expect("latest attempt metadata should remain stored");
    assert_eq!(
        observation.latest_started_attempt(),
        latest_attempt.sequence()
    );
    assert_eq!(
        observation.committed_attempt(),
        Some(latest_attempt.sequence())
    );
    assert_eq!(observation.observed_unix_seconds(), Some(1_100));
}

#[tokio::test]
async fn expired_committed_zero_weekly_window_does_not_signal_floor_after_superseded_read() {
    let test_root = TestRoot::new("quota-refresh-expired-zero-floor-intent");
    must_ok(fs::create_dir(test_root.path()));
    let state_path = test_root.path().join("state.sqlite");
    let state = must_ok(AsyncSqliteStateStore::open(&state_path).await);
    let account_id = account_id("acct_expired_zero_floor_intent");
    must_ok(
        state
            .upsert_account(
                &AccountRecord::new(
                    codex_router_core::provider::Provider::Openai,
                    account_id.clone(),
                    "expired-zero-floor-intent",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await,
    );
    let floor = WeeklyQuotaFloorBasisPoints::new(500).expect("5 percent floor should be valid");
    let mutation = must_ok(AsyncWeeklyQuotaFloorMutationStore::open(&state_path).await);
    must_ok(
        mutation
            .set_weekly_quota_floor_by_account_id(&account_id, Some(floor))
            .await,
    );
    mutation.close().await;

    let listener = must_ok(tokio::net::TcpListener::bind("127.0.0.1:0").await);
    let address = must_ok(listener.local_addr());
    let stop_server = Arc::new(tokio::sync::Notify::new());
    let server_stop = Arc::clone(&stop_server);
    let usage_request_started = Arc::new(tokio::sync::Notify::new());
    let server_usage_request_started = Arc::clone(&usage_request_started);
    let release_usage_response = Arc::new(tokio::sync::Notify::new());
    let server_release_usage_response = Arc::clone(&release_usage_response);
    let provider_server = tokio::spawn(async move {
        let mut requests = Vec::new();
        let mut held_first_usage_response = false;
        loop {
            tokio::select! {
                incoming = listener.accept() => {
                    let (mut stream, _peer_address) = incoming
                        .unwrap_or_else(|error| panic!("loopback quota provider should accept: {error}"));
                    let request = read_floor_http_request(&mut stream).await;
                    let is_usage_request = request.starts_with("GET /api/codex/usage HTTP/1.1\r\n");
                    let is_reset_request = request.starts_with("GET /api/codex/rate-limit-reset-credits HTTP/1.1\r\n");
                    assert!(
                        is_usage_request || is_reset_request,
                        "unexpected loopback quota request: {request:?}"
                    );
                    assert!(
                        request.to_ascii_lowercase().contains("authorization: bearer generation-one-token\r\n"),
                        "the in-flight provider request should use its resolved credential: {request:?}"
                    );
                    requests.push(request);
                    if is_usage_request && !held_first_usage_response {
                        held_first_usage_response = true;
                        server_usage_request_started.notify_one();
                        server_release_usage_response.notified().await;
                    }
                    let body = if is_usage_request {
                        expired_zero_weekly_usage_body()
                    } else {
                        "{}"
                    };
                    write_floor_http_response(&mut stream, body).await;
                }
                () = server_stop.notified() => return requests,
            }
        }
    });

    let resolver = FloorSignalCredentialResolver;
    let observer = Arc::new(RecordingWeeklyFloorObserver::default());
    let refresh_observer = Arc::clone(&observer);
    let secret_root = test_root.path().join("unused-secrets");
    let provider = must_ok(HttpQuotaRefreshProvider::new());
    let refresh_state_path = state_path.clone();
    let mut refresh = tokio::spawn(async move {
        let mut output = Vec::new();
        let result = refresh_quota_store_paths_with_dependencies_and_floor_notifier_async(
            &mut output,
            &refresh_state_path,
            &secret_root,
            format!("http://{address}"),
            &resolver,
            &provider,
            QuotaRefreshObservationContext {
                observed_unix_seconds: 1_100,
                schedule: QuotaRefreshSchedule::Background {
                    interval_seconds: 180,
                },
                weekly_floor_observer: Some(refresh_observer.as_ref()),
            },
        )
        .await;
        (result, output)
    });

    let usage_request_started = usage_request_started.notified();
    tokio::pin!(usage_request_started);
    tokio::select! {
        () = &mut usage_request_started => {}
        result = &mut refresh => panic!(
            "refresh should pause at the older Responses provider read: {result:?}"
        ),
    }

    let latest_attempt = state
        .begin_credit_refresh_attempt(&account_id, 1)
        .await
        .expect("newer same-generation refresh should allocate");
    let expired_window = [persisted_responses_window(&account_id, 604_800, 0, 1_000)];
    let expired_history = [successful_responses_history_observation(
        &account_id,
        604_800,
        0,
        1_000,
    )];
    state
        .record_responses_refresh_success(ResponsesRefreshSuccessCommit {
            attempt: &latest_attempt,
            selector_windows: &expired_window,
            observed_unix_seconds: 1_000,
            stale_after_unix_seconds: 1_001,
            provider_observation: &CreditProviderObservation::missing(),
            history_observations: &expired_history,
            snapshot: &successful_responses_snapshot(&account_id, 0, 1_000),
        })
        .await
        .expect("newer expired zero observation should commit before the older read returns");
    assert_eq!(latest_attempt.sequence(), 2);
    release_usage_response.notify_one();

    let (refresh_result, _output) = refresh
        .await
        .unwrap_or_else(|error| panic!("background refresh should complete: {error}"));
    stop_server.notify_one();
    let requests = provider_server
        .await
        .unwrap_or_else(|error| panic!("loopback provider task should complete: {error}"));

    must_ok(refresh_result);
    assert_eq!(requests.len(), 4, "both route bands should use HTTP reads");
    assert_eq!(
        *lock_test_mutex(&observer.account_ids, "weekly floor accounts"),
        Vec::<AccountId>::new(),
        "expired zero quota evidence must not notify the floor observer"
    );
    assert_eq!(
        *lock_test_mutex(&observer.intents, "weekly floor intents"),
        Vec::<WeeklyQuotaFloorIntent>::new(),
        "stale committed zero quota must not create a HardStop floor intent"
    );

    let observation = state
        .load_account_credit_observation(&account_id)
        .await
        .expect("latest credit observation should load")
        .expect("latest attempt metadata should remain stored");
    assert_eq!(
        observation.latest_started_attempt(),
        latest_attempt.sequence()
    );
    assert_eq!(
        observation.committed_attempt(),
        Some(latest_attempt.sequence())
    );
    assert_eq!(observation.observed_unix_seconds(), Some(1_000));
    assert_eq!(observation.stale_after_unix_seconds(), Some(1_001));
    assert_eq!(
        observation.provider_observation().availability(),
        &codex_router_core::credit_usage::CreditAvailability::Unknown,
        "the floor check must rely on fresh quota evidence, not credit entitlement"
    );
}
