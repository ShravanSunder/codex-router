use super::*;
use agent_proxy_services::credential_runtime::AsyncProviderCredentialResolver;
use codex_router_auth::resolver::ResolvedProviderCredential;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::WindowKind;
use codex_router_state::quota_snapshot::QuotaRefreshStatusView;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const STALE_OBSERVATION_AT: u64 = 100;
const STALE_DEADLINE: u64 = 200;
const FAILURE_AT: u64 = 1_000;
const FIRST_REFRESH_AT: u64 = 2_000;
const SECOND_REFRESH_AT: u64 = 3_000;

struct SyntheticClaudeCredentialResolver;

impl AsyncProviderCredentialResolver for SyntheticClaudeCredentialResolver {
    async fn resolve_provider_credentials_async(
        &self,
        account_id: &AccountId,
        expected_provider: Provider,
    ) -> Result<ResolvedProviderCredential, CredentialResolverError> {
        if expected_provider != Provider::Claude {
            return Err(CredentialResolverError::AccountProviderMismatch);
        }
        Ok(ResolvedProviderCredential::new(
            account_id.clone(),
            SecretString::new("synthetic-claude-quota-token"),
            1,
        ))
    }
}

struct FakeClaudeUsageEndpoint {
    endpoint: String,
    request_receiver: mpsc::Receiver<String>,
    server_thread: JoinHandle<()>,
}

impl FakeClaudeUsageEndpoint {
    fn finish(self) -> String {
        let request = self.request_receiver.recv_timeout(Duration::from_secs(3));
        let server_result = self.server_thread.join();
        server_result.expect("fake Claude usage server should finish");
        request
            .expect("fake Claude usage server should capture one request")
            .to_ascii_lowercase()
    }
}

fn start_fake_claude_usage_endpoint(body: String) -> FakeClaudeUsageEndpoint {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake Claude usage listener");
    listener
        .set_nonblocking(true)
        .expect("fake Claude usage listener should use bounded accept");
    let address = listener.local_addr().expect("listener address");
    let endpoint = format!("http://{address}/api/oauth/usage");
    let (request_sender, request_receiver) = mpsc::channel();
    let server_thread = thread::spawn(move || {
        let accept_deadline = Instant::now() + Duration::from_secs(3);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < accept_deadline =>
                {
                    thread::yield_now();
                }
                Err(error) => panic!("fake Claude usage server should accept: {error}"),
            }
        };
        stream
            .set_nonblocking(false)
            .expect("accepted fake usage stream should use blocking reads");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("fake usage stream should have bounded reads");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let count = stream.read(&mut buffer).expect("read fake usage request");
            assert!(count > 0, "fake usage request includes headers");
            request.extend_from_slice(&buffer[..count]);
            assert!(request.len() <= 8_192, "fake usage headers remain bounded");
            if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
        };
        request_sender
            .send(String::from_utf8(request[..header_end].to_vec()).expect("request headers"))
            .expect("test should receive fake usage request");
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write fake Claude usage response");
    });
    FakeClaudeUsageEndpoint {
        endpoint,
        request_receiver,
        server_thread,
    }
}

struct ClaudeQuotaIngestionFixture {
    _test_root: TempDir,
    router_root: PathBuf,
    account_id: AccountId,
}

impl ClaudeQuotaIngestionFixture {
    async fn new_with_stale_parse_failure() -> Self {
        let test_root = TempDir::new().expect("Claude quota ingestion root");
        let router_root = test_root.path().join("router");
        std::fs::create_dir_all(&router_root).expect("router root");
        let account_id = AccountId::new("claude_quota_ingestion").expect("Claude account id");
        let state = AsyncSqliteStateStore::open(&router_root.join("state.sqlite"))
            .await
            .expect("state store should open");
        state
            .upsert_account(
                &AccountRecord::new(
                    Provider::Claude,
                    account_id.clone(),
                    "claude-quota-ingestion",
                    AccountStatus::Enabled,
                )
                .with_active_credential_generation(1),
            )
            .await
            .expect("synthetic Claude account should persist");
        let stale_weekly_zero = WindowObservation::new(
            WindowObservationProps::new(
                account_id.clone(),
                WindowKind::Weekly,
                0,
                STALE_OBSERVATION_AT,
            )
            .with_reset_unix_seconds(500)
            .with_fresh_until_unix_seconds(STALE_DEADLINE),
        )
        .expect("stale zero-headroom observation should validate");
        state
            .record_window_observation(&stale_weekly_zero, || STALE_OBSERVATION_AT)
            .await
            .expect("stale weekly observation should persist");
        state
            .record_refresh_failure_preserving_selector_windows(
                &account_id,
                RouteBand::ClaudeMessages.as_str(),
                FAILURE_AT,
                QuotaRefreshErrorClass::ParseError,
            )
            .await
            .expect("prior parse failure should persist");
        state.close().await.expect("state store should close");

        Self {
            _test_root: test_root,
            router_root,
            account_id,
        }
    }

    async fn refresh_from_http(
        &self,
        body: String,
        observed_unix_seconds: u64,
    ) -> (
        Result<QuotaRefreshReport, QuotaCommandError>,
        String,
        String,
    ) {
        let usage_endpoint = start_fake_claude_usage_endpoint(body);
        let provider = HttpQuotaRefreshProvider::new_with_claude_usage_endpoint_for_test(
            Duration::from_secs(2),
            usage_endpoint.endpoint.clone(),
        )
        .expect("HTTP quota provider should build");
        let mut stdout = Vec::new();
        let result = refresh_quota_with_dependencies(
            &mut stdout,
            self.router_root.clone(),
            "unused-openai-base-url".to_owned(),
            &SyntheticClaudeCredentialResolver,
            &provider,
            observed_unix_seconds,
        )
        .await
        .map_err(QuotaCommandError::from);
        let output = String::from_utf8(stdout).expect("refresh output should be UTF-8");
        let request = usage_endpoint.finish();
        (result, output, request)
    }

    async fn report(&self, now_unix_seconds: u64) -> QuotaStatusReport {
        load_quota_status_report_with_availability_async(
            &self.router_root,
            false,
            now_unix_seconds,
            false,
            CredentialStoreAvailability::Ready,
        )
        .await
        .expect("quota status report should load persisted state")
    }

    async fn observations_and_status(
        &self,
    ) -> (Vec<WindowObservation>, Option<QuotaRefreshStatusView>) {
        let state = AsyncSqliteStateStore::open_read_only(&self.router_root.join("state.sqlite"))
            .await
            .expect("read-only state store should open");
        let observations = state
            .window_observations_for_account(&self.account_id)
            .await
            .expect("Claude observations should load");
        let status = state
            .quota_refresh_statuses_for_route_band(RouteBand::ClaudeMessages.as_str())
            .await
            .expect("Claude refresh status should load")
            .into_iter()
            .find(|status| status.account_id() == &self.account_id);
        state.close().await.expect("read-only state should close");
        (observations, status)
    }
}

fn mixed_claude_usage_body(weekly_percent: u32) -> String {
    format!(
        r#"{{"five_hour":{{"utilization":25,"resets_at":"2026-10-01T00:00:00Z"}},"seven_day":{{"utilization":80,"resets_at":"2026-10-02T00:00:00Z"}},"limits":[{{"kind":"session","percent":60,"resets_at":"2026-10-03T00:00:00Z"}},{{"kind":"weekly_all","percent":{weekly_percent},"resets_at":"2026-10-04T00:00:00Z"}}]}}"#
    )
}

fn assert_synthetic_claude_usage_request(request: &str) {
    assert!(request.starts_with("get /api/oauth/usage http/1.1\r\n"));
    assert!(request.contains("authorization: bearer synthetic-claude-quota-token\r\n"));
    assert!(request.contains("anthropic-beta: oauth-2025-04-20\r\n"));
}

#[tokio::test]
async fn real_http_refresh_replaces_stale_weekly_zero_and_clears_parse_failure() {
    let fixture = ClaudeQuotaIngestionFixture::new_with_stale_parse_failure().await;
    let failed_report = fixture.report(current_unix_seconds()).await;
    let failed_row = failed_report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("Claude parse-failure row");
    assert!(failed_row.updated.contains("no success"));
    assert!(failed_row.updated.contains(": parse"));
    let stale_weekly = failed_row
        .windows
        .iter()
        .find(|window| window.window_seconds == V1_WEEKLY_WINDOW_SECONDS)
        .expect("old weekly observation should remain visible after failure");
    assert_eq!(stale_weekly.remaining_headroom, 0);
    assert_eq!(stale_weekly.status, QuotaWindowStatus::Stale);

    let (result, stdout, request) = fixture
        .refresh_from_http(mixed_claude_usage_body(35), FIRST_REFRESH_AT)
        .await;
    result.expect("mixed Claude usage response should refresh successfully");
    assert_eq!(stdout, "refreshed: 1\n");
    assert_synthetic_claude_usage_request(&request);

    let (observations, status) = fixture.observations_and_status().await;
    assert_eq!(observations.len(), 2);
    let status = status.expect("successful Claude refresh status should persist");
    assert_eq!(status.last_error_class(), None);
    let observation_started_at = status
        .last_success_unix_seconds()
        .expect("successful refresh should have a timestamp");
    assert_eq!(
        status.last_attempt_unix_seconds(),
        Some(observation_started_at)
    );

    let accepted_report = fixture.report(observation_started_at).await;
    let accepted_row = accepted_report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("recovered Claude status row");
    assert!(
        accepted_row.updated.starts_with("ok "),
        "{}",
        accepted_row.updated
    );
    assert!(!accepted_row.updated.contains("failed"));
    assert_eq!(accepted_row.freshness, QuotaEvidenceFreshness::Fresh);
    assert_eq!(accepted_row.windows.len(), 2);
    for (window_seconds, expected_headroom, expected_reset) in [
        (V1_SHORT_WINDOW_SECONDS, 40, 1_790_985_600),
        (V1_WEEKLY_WINDOW_SECONDS, 65, 1_791_072_000),
    ] {
        let window = accepted_row
            .windows
            .iter()
            .find(|window| window.window_seconds == window_seconds)
            .expect("selected mixed-response window should appear in report");
        assert_eq!(window.remaining_headroom, expected_headroom);
        assert_eq!(window.reset_unix_seconds, Some(expected_reset));
        assert_eq!(window.observed_unix_seconds, observation_started_at);
        assert_eq!(window.status, QuotaWindowStatus::Eligible);
    }

    for (window_kind, expected_headroom, expected_reset) in [
        (WindowKind::FiveHour, 4_000, 1_790_985_600),
        (WindowKind::Weekly, 6_500, 1_791_072_000),
    ] {
        let observation = observations
            .iter()
            .find(|observation| observation.window_kind() == window_kind)
            .expect("selected window should persist in SQLite");
        assert_eq!(observation.remaining_basis_points(), expected_headroom);
        assert_eq!(observation.reset_unix_seconds(), Some(expected_reset));
        assert_eq!(observation.observation_started_at(), observation_started_at);
    }
}

#[tokio::test]
async fn real_http_refresh_persists_structured_weekly_exhaustion_as_zero_headroom() {
    let fixture = ClaudeQuotaIngestionFixture::new_with_stale_parse_failure().await;
    let (result, stdout, request) = fixture
        .refresh_from_http(mixed_claude_usage_body(100), SECOND_REFRESH_AT)
        .await;
    result.expect("exhausted weekly Claude response should refresh successfully");
    assert_eq!(stdout, "refreshed: 1\n");
    assert_synthetic_claude_usage_request(&request);

    let (observations, status) = fixture.observations_and_status().await;
    let status = status.expect("successful exhausted refresh status should persist");
    assert_eq!(status.last_error_class(), None);
    let observation_started_at = status
        .last_success_unix_seconds()
        .expect("successful refresh should have a timestamp");
    let report = fixture.report(observation_started_at).await;
    let row = report
        .rows()
        .iter()
        .find(|row| row.account_id == fixture.account_id)
        .expect("exhausted Claude report row");
    assert!(row.updated.starts_with("ok "), "{}", row.updated);
    assert_eq!(row.freshness, QuotaEvidenceFreshness::Fresh);
    let weekly = row
        .windows
        .iter()
        .find(|window| window.window_seconds == V1_WEEKLY_WINDOW_SECONDS)
        .expect("exhausted weekly observation should appear");
    assert_eq!(weekly.remaining_headroom, 0);
    assert_eq!(weekly.reset_unix_seconds, Some(1_791_072_000));
    assert_eq!(weekly.observed_unix_seconds, observation_started_at);
    assert_eq!(weekly.status, QuotaWindowStatus::Eligible);

    let weekly_observation = observations
        .iter()
        .find(|observation| observation.window_kind() == WindowKind::Weekly)
        .expect("exhausted weekly window should persist in SQLite");
    assert_eq!(weekly_observation.remaining_basis_points(), 0);
    assert_eq!(weekly_observation.reset_unix_seconds(), Some(1_791_072_000));
    assert_eq!(
        weekly_observation.observation_started_at(),
        observation_started_at
    );
}
