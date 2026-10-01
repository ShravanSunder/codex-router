//! Real listener, encrypted credentials, SQLite pins, and fake Anthropic proof.

use super::*;
use crate::http_sse::HttpProxyRequest;
use crate::server::read_http_request;
use codex_router_core::ids::AccountId;
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::TcpListener as TokioTcpListener;
use tokio::sync::Notify;
use tokio::sync::oneshot;

#[path = "server_pipeline_tests/acceptance_regressions.rs"]
mod acceptance_regressions;
use acceptance_regressions::run_claude_loopback_scenario;

fn assert_no_credential_patterns(
    surfaces: &[(&str, &str)],
    credential_patterns: &[(String, String)],
) {
    for (surface_name, surface) in surfaces {
        for (pattern_name, pattern) in credential_patterns {
            assert!(
                !surface.contains(pattern),
                "{surface_name} exposed {pattern_name}"
            );
        }
    }
}

struct BlockingPassiveQuotaObservationWriter {
    started_sender: std::sync::Mutex<Option<oneshot::Sender<u64>>>,
    release_writer: Notify,
}

impl crate::http_sse::AsyncClaudeQuotaObservationWriter for BlockingPassiveQuotaObservationWriter {
    fn record_window_observation<'a>(
        &'a self,
        observation: codex_router_state::window_observation::WindowObservation,
    ) -> futures_util::future::BoxFuture<'a, Result<(), codex_router_state::sqlite::StateStoreError>>
    {
        Box::pin(async move {
            let started_sender = self
                .started_sender
                .lock()
                .unwrap_or_else(|_| panic!("writer start mutex is not poisoned"))
                .take();
            if let Some(started_sender) = started_sender {
                let _sent = started_sender.send(observation.observation_started_at());
            }
            self.release_writer.notified().await;
            Ok(())
        })
    }
}

#[derive(Clone, Copy)]
enum FakeClaudeOAuthIssuerOutcome {
    Rotated,
    Refused,
    Uncertain,
}

struct FakeClaudeOAuthIssuer {
    endpoint: String,
    server_thread: std::thread::JoinHandle<HttpProxyRequest>,
}

impl FakeClaudeOAuthIssuer {
    fn start(outcome: FakeClaudeOAuthIssuerOutcome) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("fake OAuth listener");
        listener
            .set_nonblocking(true)
            .expect("nonblocking fake OAuth listener");
        let endpoint = format!(
            "http://{}/v1/oauth/token",
            listener.local_addr().expect("fake OAuth address")
        );
        let server_thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("fake OAuth runtime");
            runtime.block_on(async move {
                let listener = TokioTcpListener::from_std(listener).expect("async OAuth listener");
                let (stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                    .await
                    .expect("Claude refresh reaches fake issuer")
                    .expect("fake OAuth accept");
                let mut stream = stream.into_std().expect("fake OAuth stream");
                stream.set_nonblocking(false).expect("blocking OAuth stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("OAuth request timeout");
                let request = read_http_request(&mut stream).expect("OAuth refresh request");
                let response = match outcome {
                    FakeClaudeOAuthIssuerOutcome::Rotated => Some(claude_fixture_response(
                        "200 OK",
                        "Content-Type: application/json\r\n",
                        r#"{"access_token":"claude-access-rotated","refresh_token":"claude-refresh-rotated","expires_in":3600}"#,
                    )),
                    FakeClaudeOAuthIssuerOutcome::Refused => Some(claude_fixture_response(
                        "400 Bad Request",
                        "Content-Type: application/json\r\n",
                        r#"{"error":"invalid_grant"}"#,
                    )),
                    FakeClaudeOAuthIssuerOutcome::Uncertain => Some(format!(
                        "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{{\"access_token\":",
                        100
                    )),
                };
                if let Some(response) = response {
                    stream
                        .write_all(response.as_bytes())
                        .expect("fake OAuth response");
                }
                request
            })
        });
        Self {
            endpoint,
            server_thread,
        }
    }

    fn finish(self) -> HttpProxyRequest {
        self.server_thread.join().expect("fake OAuth server join")
    }
}

fn claude_credential_rejection() -> String {
    claude_fixture_response(
        "401 Unauthorized",
        "Content-Type: application/json\r\n",
        r#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired. Please obtain a new token or refresh your existing token."}}"#,
    )
}

fn assert_fake_claude_refresh_request(request: &HttpProxyRequest, expected_refresh_token: &str) {
    assert_eq!(request.path(), "/v1/oauth/token");
    assert!(
        request
            .header_value("content-type")
            .is_some_and(|value| { value.eq_ignore_ascii_case("application/json") })
    );
    let body = serde_json::from_slice::<serde_json::Value>(request.body())
        .expect("fake issuer receives JSON refresh contract");
    assert!(
        body.get("grant_type").and_then(serde_json::Value::as_str) == Some("refresh_token"),
        "refresh request uses the token grant"
    );
    assert!(
        body.get("refresh_token")
            .and_then(serde_json::Value::as_str)
            == Some(expected_refresh_token),
        "refresh request carries the rejected account refresh token"
    );
    assert!(
        body.get("client_id").and_then(serde_json::Value::as_str) == Some("test-claude-client"),
        "refresh request uses the configured Claude client id"
    );
}

fn read_claude_credential_maintenance(
    receipt: &ClaudeLoopbackReceipt,
    account_id: &AccountId,
) -> codex_router_state::credential_maintenance::CredentialMaintenanceRecord {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("maintenance read runtime")
        .block_on(async {
            AsyncSqliteStateStore::open(&receipt.database_path)
                .await
                .expect("maintenance read state")
                .load_credential_maintenance(account_id)
                .await
                .expect("maintenance state read")
                .expect("credential maintenance record")
        })
}

struct ClaudeLoopbackScenario {
    case: &'static str,
    request_body: Vec<u8>,
    reserve_primary: bool,
    reserve_primary_while_streaming: bool,
    quota_refresh_interval: Duration,
    responses: Vec<String>,
}

struct ClaudeLoopbackReceipt {
    response: String,
    second_response: Option<String>,
    logs: String,
    requests: Vec<HttpProxyRequest>,
    oauth_request: Option<HttpProxyRequest>,
    database_path: PathBuf,
    _temporary_directory: tempfile::TempDir,
    account_ids: Vec<AccountId>,
    credential_patterns: Vec<(String, String)>,
}

fn claude_fixture_response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn write_claude_fixture_chunk(stream: &mut TcpStream, body: &str) {
    write!(stream, "{:X}\r\n", body.len()).expect("chunk size");
    stream.write_all(body.as_bytes()).expect("chunk body");
    stream.write_all(b"\r\n").expect("chunk terminator");
}

fn read_until_response_marker(stream: &mut TcpStream, marker: &[u8]) -> Vec<u8> {
    let mut response = Vec::new();
    while !response
        .windows(marker.len())
        .any(|window| window == marker)
    {
        let mut byte = [0_u8; 1];
        stream
            .read_exact(&mut byte)
            .unwrap_or_else(|error| panic!("stream should reach marker: {error}"));
        response.push(byte[0]);
    }
    response
}

fn write_claude_fixture_request(client: &mut TcpStream, request_body: &[u8]) {
    write_claude_fixture_request_with_declared_content_length(
        client,
        request_body,
        request_body.len(),
    );
}

fn write_claude_fixture_request_with_declared_content_length(
    client: &mut TcpStream,
    request_body: &[u8],
    content_length: usize,
) {
    write!(client, "POST /anthropic/v1/messages?trace=fixture HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {content_length}\r\nAuthorization: Bearer claude-local\r\nX-Api-Key: client-key\r\nX-Claude-Code-Session-Id: claude-session\r\nAnthropic-Version: 2023-06-01\r\nAnthropic-Beta: client-capability\r\n\r\n").expect("request headers");
    client.write_all(request_body).expect("request body");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("request complete");
}

fn mark_claude_primary_reserve(database_path: &std::path::Path, account_id: &AccountId) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("quota update runtime")
        .block_on(async {
            let state = AsyncSqliteStateStore::open(database_path)
                .await
                .expect("quota update state");
            let observation = codex_router_state::window_observation::WindowObservation::new(
                codex_router_state::window_observation::WindowObservationProps::new(
                    account_id.clone(),
                    codex_router_core::route_profile::WindowKind::FiveHour,
                    500,
                    1_099,
                )
                .with_reset_unix_seconds(2_000)
                .with_fresh_until_unix_seconds(2_000),
            )
            .expect("reserve observation");
            assert!(
                state
                    .record_window_observation(&observation, || 1_100)
                    .await
                    .expect("record reserve observation")
            );
        });
}

fn client_response_body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("fixture response has headers and a body boundary"))
}

fn read_claude_fixture_pin(
    receipt: &ClaudeLoopbackReceipt,
) -> codex_router_state::session_account_affinity::SessionAccountAffinity {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("read runtime")
        .block_on(async {
            AsyncSqliteStateStore::open(&receipt.database_path)
                .await
                .expect("read state")
                .load_session_account_affinity(
                    codex_router_core::provider::Provider::Claude,
                    "claude-session",
                )
                .await
                .expect("read pin")
                .expect("fixture pin")
        })
}

#[test]
fn claude_server_success_preserves_request_and_renews_pin_after_body_completion() {
    let body = br#"{"model":"claude-sonnet","messages":[]}"#.to_vec();
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        case: "claude_server_success",
        request_body: body.clone(),
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![claude_fixture_response(
            "200 OK",
            "Content-Type: application/json\r\n",
            "{\"type\":\"message\"}",
        )],
    });
    assert!(
        receipt.response.starts_with("HTTP/1.1 200 OK"),
        "{}",
        receipt.response
    );
    assert_eq!(receipt.requests.len(), 1);
    let request = &receipt.requests[0];
    assert_eq!(request.path(), "/v1/messages?trace=fixture");
    assert_eq!(request.body(), body);
    assert_eq!(
        request.header_value("authorization"),
        Some("Bearer claude-access-0")
    );
    assert_eq!(request.header_value("x-api-key"), None);
    assert_eq!(
        request.header_value("anthropic-beta"),
        Some("client-capability,oauth-2025-04-20")
    );
    assert_eq!(
        request.header_value("anthropic-version"),
        Some("2023-06-01")
    );
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[0]));
    assert_eq!(pin.pin_version(), 3);
    assert_eq!(pin.last_seen_unix_seconds(), 1_100);
}

#[test]
fn claude_server_persists_passive_windows_with_the_configured_interval_deadline() {
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        case: "claude_server_passive_quota_freshness",
        request_body: b"{}".to_vec(),
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        quota_refresh_interval: Duration::from_secs(400),
        responses: vec![claude_fixture_response(
            "200 OK",
            "anthropic-ratelimit-unified-5h-utilization: 0.25\r\nanthropic-ratelimit-unified-5h-reset: 3000\r\nanthropic-ratelimit-unified-7d-utilization: 0.8\r\nanthropic-ratelimit-unified-7d-reset: 4000\r\n",
            "{}",
        )],
    });
    assert!(receipt.response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(receipt.requests.len(), 1);

    let observations = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("read runtime")
        .block_on(async {
            AsyncSqliteStateStore::open(&receipt.database_path)
                .await
                .expect("read state")
                .window_observations_for_account(&receipt.account_ids[0])
                .await
                .expect("read passive observations")
        });
    assert_eq!(observations.len(), 2);
    for observation in &observations {
        assert_eq!(observation.observation_started_at(), 1_100);
        assert_eq!(observation.fresh_until_unix_seconds(), Some(1_620));
    }
    assert!(observations.iter().any(|observation| {
        observation.window_kind() == codex_router_core::route_profile::WindowKind::FiveHour
    }));
    assert!(observations.iter().any(|observation| {
        observation.window_kind() == codex_router_core::route_profile::WindowKind::Weekly
    }));
}

fn claude_shared_window_rejection() -> String {
    claude_fixture_response(
        "429 Too Many Requests",
        "anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-representative-claim: five_hour\r\nanthropic-ratelimit-unified-5h-reset: 2000\r\n",
        "{\"type\":\"error\"}",
    )
}

#[test]
fn claude_server_shared_window_rejection_replays_body_and_publishes_replacement_pin() {
    let body = vec![b' '; 3 * 1024 * 1024];
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        case: "claude_server_retry",
        request_body: body.clone(),
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![
            claude_shared_window_rejection(),
            claude_fixture_response(
                "200 OK",
                "Content-Type: application/json\r\n",
                "{\"type\":\"message\"}",
            ),
        ],
    });
    assert!(
        receipt.response.starts_with("HTTP/1.1 200 OK"),
        "{}",
        receipt.response
    );
    assert_eq!(receipt.requests.len(), 2);
    for request in &receipt.requests {
        assert_eq!(request.body(), body);
    }
    assert_eq!(
        receipt.requests[0].header_value("authorization"),
        Some("Bearer claude-access-0")
    );
    assert_eq!(
        receipt.requests[1].header_value("authorization"),
        Some("Bearer claude-access-1")
    );
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[1]));
    assert_eq!(pin.pin_version(), 5);
}

#[test]
fn claude_server_final_credential_rejection_marks_generation_without_third_attempt() {
    let rejection = r#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired"}}"#;
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        case: "claude_server_final_rejection",
        request_body: b"{}".to_vec(),
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![
            claude_shared_window_rejection(),
            claude_fixture_response("401 Unauthorized", "", rejection),
        ],
    });
    assert!(
        receipt.response.starts_with("HTTP/1.1 401 Unauthorized"),
        "{}",
        receipt.response
    );
    assert!(receipt.response.ends_with(rejection));
    assert_eq!(receipt.requests.len(), 2);
    let maintenance = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("read runtime")
        .block_on(async {
            AsyncSqliteStateStore::open(&receipt.database_path)
                .await
                .expect("read state")
                .load_credential_maintenance(&receipt.account_ids[1])
                .await
                .expect("read maintenance")
                .expect("rejection state")
        });
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
    assert_eq!(maintenance.credential_generation, 1);
    assert_eq!(read_claude_fixture_pin(&receipt).account_id(), None);
}

#[test]
fn claude_server_pass_through_preserves_long_error_body_without_renewing_pin() {
    let body = "provider-error".repeat(6_000);
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        case: "claude_server_pass_through",
        request_body: b"{}".to_vec(),
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![claude_fixture_response(
            "529 Overloaded",
            "X-Fixture: retained\r\n",
            &body,
        )],
    });
    assert!(receipt.response.starts_with("HTTP/1.1 529"));
    assert!(receipt.response.ends_with(&body));
    assert!(receipt.response.contains("x-fixture: retained"));
    assert_eq!(receipt.requests.len(), 1);
    assert_eq!(
        read_claude_fixture_pin(&receipt).last_seen_unix_seconds(),
        1_000
    );
}

#[test]
fn claude_server_incomplete_success_stream_does_not_renew_pin() {
    let body = "event: message_start\ndata: {\"type\":\"message_start\"}\n\n";
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        case: "claude_server_incomplete",
        request_body: b"{}".to_vec(),
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![claude_fixture_response(
            "200 OK",
            "Content-Type: text/event-stream\r\n",
            body,
        )],
    });
    assert!(receipt.response.starts_with("HTTP/1.1 200 OK"));
    assert!(receipt.response.ends_with(body));
    assert_eq!(receipt.requests.len(), 1);
    assert_eq!(
        read_claude_fixture_pin(&receipt).last_seen_unix_seconds(),
        1_000
    );
}

#[test]
fn claude_server_complete_sse_message_stop_renews_pin() {
    let body = "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        case: "claude_server_sse_complete",
        request_body: b"{}".to_vec(),
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![claude_fixture_response(
            "200 OK",
            "Content-Type: text/event-stream\r\n",
            body,
        )],
    });
    assert!(receipt.response.starts_with("HTTP/1.1 200 OK"));
    assert!(receipt.response.ends_with(body));
    assert_eq!(receipt.requests.len(), 1);
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.pin_version(), 3);
    assert_eq!(pin.last_seen_unix_seconds(), 1_100);
}

#[test]
fn claude_server_reserve_pin_releases_through_writable_pool_before_preferred_attempt() {
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        case: "claude_server_reserve_release",
        request_body: b"{}".to_vec(),
        reserve_primary: true,
        reserve_primary_while_streaming: false,
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![claude_fixture_response(
            "200 OK",
            "Content-Type: application/json\r\n",
            "{}",
        )],
    });
    assert!(
        receipt.response.starts_with("HTTP/1.1 200 OK"),
        "{}",
        receipt.response
    );
    assert_eq!(receipt.requests.len(), 1);
    assert_eq!(
        receipt.requests[0].header_value("authorization"),
        Some("Bearer claude-access-1")
    );
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[1]));
    assert_eq!(pin.pin_version(), 5);
}

#[tokio::test]
async fn passive_quota_writer_block_does_not_delay_upstream_response_completion() {
    let response = AsyncStreamingHttpProxyResponse::new(
        200,
        HeaderCollection::new(vec![
            Header::new("content-type", "application/json"),
            Header::new("anthropic-ratelimit-unified-5h-utilization", "0.25"),
            Header::new("anthropic-ratelimit-unified-5h-reset", "900"),
        ]),
        http_body_util::Full::new(Bytes::from_static(b"upstream response"))
            .map_err(|never| -> crate::http_sse::AsyncHttpBodyError { match never {} })
            .boxed(),
    );
    let (started_sender, started_receiver) = oneshot::channel();
    let writer = Arc::new(BlockingPassiveQuotaObservationWriter {
        started_sender: std::sync::Mutex::new(Some(started_sender)),
        release_writer: Notify::new(),
    });
    let task_tracker = TaskTracker::new();
    let response = schedule_passive_claude_quota_observation(
        response,
        writer.clone(),
        super::r12_tests::account_id("claude-blocked-writer"),
        123,
        Duration::from_secs(180),
        task_tracker.clone(),
    );

    let observation_started_at = tokio::time::timeout(Duration::from_secs(1), started_receiver)
        .await
        .unwrap_or_else(|_| panic!("passive writer should start within the bound"))
        .unwrap_or_else(|_| panic!("passive writer start signal should remain open"));
    assert_eq!(observation_started_at, 123);

    let (_, _, response_body) = response.into_parts();
    let response_bytes = tokio::time::timeout(Duration::from_secs(1), response_body.collect())
        .await
        .unwrap_or_else(|_| {
            panic!("upstream response should finish while its quota writer is blocked")
        })
        .unwrap_or_else(|error| panic!("upstream response should be readable: {error}"))
        .to_bytes();
    assert_eq!(response_bytes.as_ref(), b"upstream response");

    writer.release_writer.notify_one();
    task_tracker.close();
    tokio::time::timeout(Duration::from_secs(1), task_tracker.wait())
        .await
        .unwrap_or_else(|_| panic!("released passive writer task should finish within the bound"));
}
