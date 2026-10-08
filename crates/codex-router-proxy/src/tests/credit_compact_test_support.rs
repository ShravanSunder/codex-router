use super::*;

use std::io::Write;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;
use std::time::Duration;

pub(super) struct CompactFixture {
    _temporary_directory: ProxyTestTempDir,
    pub(super) database_path: std::path::PathBuf,
    secret_path: std::path::PathBuf,
    secrets: EncryptedCredentialStore,
    pub(super) now_unix_seconds: u64,
}

impl CompactFixture {
    pub(super) fn new(name: &str) -> Self {
        let temporary_directory = ProxyTestTempDir::new(name);
        let database_path = temporary_directory.path().join("state.sqlite");
        let secret_path = temporary_directory.path().join("secrets");
        let secrets =
            codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_path)
                .expect("compact test credentials should open");
        Self {
            _temporary_directory: temporary_directory,
            database_path,
            secret_path,
            secrets,
            now_unix_seconds: test_unix_seconds(),
        }
    }

    pub(super) async fn add_credit_account(
        &self,
        account_id_value: &str,
        label: &str,
        upstream_token: &str,
        allow_credit_usage: bool,
        availability: codex_router_core::credit_usage::CreditAvailability,
    ) -> AccountRecord {
        let account = AccountRecord::new(
            Provider::Openai,
            account_id(account_id_value),
            label,
            AccountStatus::Enabled,
        );
        persist_credit_account_with_availability(
            &self.database_path,
            &self.secrets,
            &account,
            upstream_token,
            self.now_unix_seconds,
            allow_credit_usage,
            availability,
        )
        .await;
        account
    }

    pub(super) async fn assert_route(
        self,
        scenario: &str,
        expected_upstream_token: Option<&str>,
        expected_selection_reason: Option<&str>,
    ) {
        let Self {
            _temporary_directory,
            database_path,
            secret_path,
            secrets,
            now_unix_seconds,
        } = self;
        assert_compact_transport_result(
            scenario,
            &database_path,
            &secret_path,
            secrets,
            now_unix_seconds,
            expected_upstream_token,
            expected_selection_reason,
        )
        .await;
    }
}

pub(super) fn compact_available_credits(
    balance: &str,
) -> codex_router_core::credit_usage::CreditAvailability {
    codex_router_core::credit_usage::CreditAvailability::Available {
        balance: Some(
            codex_router_core::credit_usage::CreditBalance::new(balance)
                .expect("compact credit balance should parse"),
        ),
    }
}

pub(super) fn replace_selector_windows(
    database_path: &Path,
    account_id: &AccountId,
    route_band: &str,
    observed_unix_seconds: u64,
    status: SelectorQuotaWindowStatus,
    remaining_headroom: u32,
) {
    let state = SqliteStateStore::open(database_path).expect("compact selector state should open");
    let selector_windows = [
        PersistedSelectorQuotaWindow::new(account_id.clone(), route_band, 18_000, status)
            .with_remaining_headroom(remaining_headroom)
            .with_effective(true)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_reset_unix_seconds(observed_unix_seconds + 18_000),
        PersistedSelectorQuotaWindow::new(account_id.clone(), route_band, 604_800, status)
            .with_remaining_headroom(remaining_headroom)
            .with_effective(false)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_reset_unix_seconds(observed_unix_seconds + 604_800),
    ];
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &state,
        account_id,
        route_band,
        &selector_windows,
        observed_unix_seconds,
        observed_unix_seconds + 300,
    )
    .expect("compact selector windows should persist");
}

pub(super) fn clear_selector_windows(
    database_path: &Path,
    account_id: &AccountId,
    route_band: &str,
    observed_unix_seconds: u64,
) {
    let state = SqliteStateStore::open(database_path).expect("compact selector state should open");
    SelectorQuotaRepository::record_refresh_success_and_replace_selector_windows(
        &state,
        account_id,
        route_band,
        &[],
        observed_unix_seconds,
        observed_unix_seconds + 300,
    )
    .expect("empty canonical selector windows should persist");
}

pub(super) async fn mark_credit_refresh_pending(database_path: &Path, account_id: &AccountId) {
    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .expect("pending-credit state should open");
    let attempt = state
        .begin_credit_refresh_attempt(account_id, 1)
        .await
        .expect("pending-credit attempt should begin");
    assert_eq!(attempt.sequence(), 2);
    state
        .close()
        .await
        .expect("pending-credit state should close");
}

pub(super) async fn set_provider_spend_control_reached(
    database_path: &Path,
    account_id: &AccountId,
    observed_unix_seconds: u64,
) {
    let state = AsyncSqliteStateStore::open(database_path)
        .await
        .expect("spend-control state should open");
    let attempt = state
        .begin_credit_refresh_attempt(account_id, 1)
        .await
        .expect("spend-control attempt should begin");
    let windows = [
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            18_000,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(true)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 18_000),
        PersistedSelectorQuotaWindow::new(
            account_id.clone(),
            "responses",
            604_800,
            SelectorQuotaWindowStatus::Ineligible,
        )
        .with_remaining_headroom(0)
        .with_effective(false)
        .with_observed_unix_seconds(observed_unix_seconds)
        .with_reset_unix_seconds(observed_unix_seconds + 604_800),
    ];
    let history = windows
        .iter()
        .map(|window| {
            PersistedQuotaHistoryObservation::new(
                account_id.clone(),
                "compact-spend-control",
                "responses",
                window.limit_window_seconds(),
                observed_unix_seconds,
                window.remaining_headroom(),
            )
            .with_reset_unix_seconds(
                window
                    .reset_unix_seconds()
                    .expect("spend-control window should have reset time"),
            )
            .with_window_status(SelectorQuotaWindowStatus::Ineligible)
            .with_effective(window.effective())
            .with_refresh_source(QuotaSnapshotSource::OpenAiEndpoint)
            .with_refresh_outcome(QuotaHistoryRefreshOutcome::Success)
        })
        .collect::<Vec<_>>();
    let snapshot =
        PersistedQuotaSnapshot::new(account_id.clone(), QuotaSnapshotSource::OpenAiEndpoint)
            .with_observed_unix_seconds(observed_unix_seconds)
            .with_route_band("responses", 0)
            .with_reset_unix_seconds(observed_unix_seconds + 18_000)
            .with_stale_penalty(false);
    let provider_observation = codex_router_core::credit_usage::CreditProviderObservation::new(
            compact_available_credits("3.25"),
            codex_router_core::credit_usage::CreditSpendControl::Reached,
            Some(
                codex_router_core::credit_usage::CreditProviderLimitReason::
                    WorkspaceOwnerUsageLimitReached,
            ),
        );
    let committed = state
        .record_responses_refresh_success(
            codex_router_state::credit_store::ResponsesRefreshSuccessCommit {
                attempt: &attempt,
                selector_windows: &windows,
                observed_unix_seconds,
                stale_after_unix_seconds: observed_unix_seconds + 300,
                provider_observation: &provider_observation,
                history_observations: &history,
                snapshot: &snapshot,
            },
        )
        .await
        .expect("spend-control observation should commit");
    assert!(committed);
    state
        .close()
        .await
        .expect("spend-control state should close");
}

async fn assert_compact_transport_result(
    scenario: &str,
    database_path: &Path,
    secret_path: &Path,
    secrets: EncryptedCredentialStore,
    now_unix_seconds: u64,
    expected_upstream_token: Option<&str>,
    expected_selection_reason: Option<&str>,
) {
    let upstream_listener =
        TcpListener::bind("127.0.0.1:0").expect("compact mock upstream should bind");
    let upstream_address = upstream_listener
        .local_addr()
        .expect("compact upstream address should read");
    let upstream_probe = CompactUpstreamProbe::start(upstream_listener);
    let endpoint = UpstreamEndpoint::new(format!("http://{upstream_address}/v1"))
        .expect("compact upstream endpoint should validate");
    let bind_address =
        LoopbackBindAddress::new("127.0.0.1", 0).expect("compact loopback address should validate");
    let config = LoopbackRouterRuntimeConfig::new(
        bind_address,
        endpoint,
        database_path.to_owned(),
        secret_path.to_owned(),
        LocalRouterTokenRecord::new(SecretString::new("current-token"), TokenGeneration::new(1)),
    )
    .with_quota_clock(now_unix_seconds, 60);

    let (captured_logs, (handled_connections, response)) =
        crate::test_log_capture::capture_log_output_async(async {
            let runtime = LoopbackRouterRuntime::start(config, secrets)
                .await
                .expect("compact loopback runtime should start");
            let router_address = runtime.local_addr();
            let client_thread = std::thread::spawn(move || {
                send_loopback_request(
                    router_address,
                    "POST /v1/responses/compact HTTP/1.1\r\n",
                    br#"{"model":"gpt-5","compact_credit_test":true}"#,
                )
            });
            let handled_connections = runtime
                .serve_http_connections(1)
                .await
                .expect("compact request should be served");
            let response = client_thread.join().expect("compact client should finish");
            (handled_connections, response)
        })
        .await;
    assert_eq!(handled_connections, 1);

    if expected_upstream_token.is_some() {
        assert!(
            response.starts_with("HTTP/1.1 200 OK\r\n"),
            "{scenario}: {response}"
        );
        assert!(response.ends_with("\r\n\r\nok"), "{scenario}: {response}");
    } else {
        assert!(
            response.starts_with("HTTP/1.1 503 Service Unavailable\r\n"),
            "{scenario} should block the compact request without a valid fallback: {response}"
        );
        let body = response
            .split_once("\r\n\r\n")
            .map(|(_headers, body)| body)
            .expect("compact unavailable response should include a body");
        assert_eq!(body, crate::websocket::ROUTER_ALL_ACCOUNTS_EXHAUSTED_SIGNAL);
        upstream_probe.release_with_shutdown_probe();
    }

    let (request_line, authorization) = upstream_probe.receive_request();
    let expected_request_line = if expected_upstream_token.is_some() {
        "POST /v1/responses/compact HTTP/1.1"
    } else {
        "GET /__compact_probe_shutdown HTTP/1.1"
    };
    assert_eq!(
        request_line, expected_request_line,
        "{scenario} upstream request"
    );
    let expected_authorization =
        expected_upstream_token.map(|token| format!("authorization: Bearer {token}"));
    assert_eq!(
        authorization.as_deref(),
        expected_authorization.as_deref(),
        "{scenario} selected credential"
    );

    let reservation_log = captured_logs
        .lines()
        .find(|line| line.contains("codex_router.account_reserved"));
    if expected_upstream_token.is_some() {
        let reservation_log = reservation_log.unwrap_or_else(|| {
            panic!("{scenario} should record its selected reason: {captured_logs}")
        });
        assert!(
            reservation_log.contains("selection.reason"),
            "{scenario} reservation should expose its reason: {reservation_log}"
        );
        if let Some(expected_reason) = expected_selection_reason {
            assert!(
                reservation_log.contains(expected_reason),
                "{scenario} should select reason {expected_reason}: {reservation_log}"
            );
        } else {
            assert!(
                !reservation_log.contains("credit_backed"),
                "{scenario} should retain ordinary compact selection: {reservation_log}"
            );
        }
    } else {
        assert!(
            !captured_logs.contains("selection.reason=credit_backed"),
            "{scenario} must not claim credit-backed admission: {captured_logs}"
        );
    }
}

struct CompactUpstreamProbe {
    address: SocketAddr,
    receiver: Receiver<(String, Option<String>)>,
    server_thread: Option<JoinHandle<()>>,
}

impl CompactUpstreamProbe {
    fn start(listener: TcpListener) -> Self {
        let address = listener
            .local_addr()
            .expect("compact upstream address should read");
        let (sender, receiver) = mpsc::channel();
        let server_thread = std::thread::spawn(move || {
            let (mut stream, _peer_address) = listener
                .accept()
                .expect("compact upstream should accept a request");
            let request = read_test_http_request(&mut stream);
            let request_line = request.lines().next().unwrap_or("<missing>").to_owned();
            let authorization = request
                .lines()
                .find(|line| line.starts_with("authorization: "))
                .map(str::to_owned);
            sender
                .send((request_line.clone(), authorization))
                .expect("compact upstream request should be observed");
            if request_line != "GET /__compact_probe_shutdown HTTP/1.1" {
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .expect("compact provider response should be written");
            }
        });
        Self {
            address,
            receiver,
            server_thread: Some(server_thread),
        }
    }

    fn receive_request(&self) -> (String, Option<String>) {
        self.receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("compact upstream request should arrive within the bound")
    }

    fn release_with_shutdown_probe(&self) {
        let mut stream = TcpStream::connect(self.address)
            .expect("compact upstream shutdown probe should connect");
        stream
            .write_all(b"GET /__compact_probe_shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .expect("compact upstream shutdown probe should write");
    }
}

impl Drop for CompactUpstreamProbe {
    fn drop(&mut self) {
        if let Some(server_thread) = self.server_thread.take() {
            if let Ok(mut stream) = TcpStream::connect(self.address) {
                let _ = stream.write_all(
                    b"GET /__compact_probe_shutdown HTTP/1.1\r\nHost: localhost\r\n\r\n",
                );
            }
            let _ = server_thread.join();
        }
    }
}
