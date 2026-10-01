//! Real listener, encrypted credentials, SQLite pins, and fake Anthropic proof.

use super::*;
use crate::server::{
    LoopbackBindAddress, LoopbackRouterRuntime, LoopbackRouterRuntimeConfig, read_http_request,
};
use crate::upstream::{ClaudeUpstreamEndpoint, UpstreamEndpoint};
use codex_router_core::ids::AccountId;
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_state::account::{AccountRecord, AccountStatus};
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::net::TcpListener as TokioTcpListener;

static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn test_database_path(name: &str) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "codex-router-claude-server-{name}-{}-{counter}.sqlite",
        std::process::id()
    ))
}

fn local_router_token(token: &str, generation: u64) -> LocalRouterTokenRecord {
    LocalRouterTokenRecord::new(SecretString::new(token), TokenGeneration::new(generation))
}

struct ClaudeLoopbackScenario {
    case: &'static str,
    request_body: Vec<u8>,
    reserve_primary: bool,
    quota_refresh_interval: Duration,
    responses: Vec<String>,
}

struct ClaudeLoopbackReceipt {
    response: String,
    requests: Vec<HttpProxyRequest>,
    database_path: PathBuf,
    account_ids: Vec<AccountId>,
}

fn claude_fixture_response(status: &str, headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n{body}",
        body.len()
    )
}

fn run_claude_loopback_scenario(scenario: ClaudeLoopbackScenario) -> ClaudeLoopbackReceipt {
    use codex_router_core::provider::Provider;
    use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
    use codex_router_secret_store::credential_bundle::CredentialBundle;
    use codex_router_state::session_account_affinity::SessionAccountAffinity;

    let reserve_primary = scenario.reserve_primary;
    let upstream_listener = TcpListener::bind("127.0.0.1:0").expect("fixture listener");
    upstream_listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let upstream_address = upstream_listener.local_addr().expect("fixture address");
    let upstream_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("upstream runtime");
        runtime.block_on(async move {
            let listener = TokioTcpListener::from_std(upstream_listener).expect("async listener");
            let mut requests = Vec::new();
            for response in scenario.responses {
                let Ok(accepted) =
                    tokio::time::timeout(Duration::from_secs(3), listener.accept()).await
                else {
                    break;
                };
                let (stream, _) = accepted.expect("upstream accept");
                let mut stream = stream.into_std().expect("upstream stream");
                stream.set_nonblocking(false).expect("blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .expect("read timeout");
                requests.push(read_http_request(&mut stream).expect("upstream request"));
                stream
                    .write_all(response.as_bytes())
                    .expect("upstream response");
            }
            requests
        })
    });
    let database_path = test_database_path(scenario.case);
    let secret_root = database_path.with_extension("secrets");
    let secrets =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .expect("fixture secrets");
    let state_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("state runtime");
    let account_ids = vec![
        AccountId::new("claude-primary").expect("account"),
        AccountId::new("claude-secondary").expect("account"),
    ];
    for (index, account_id) in account_ids.iter().enumerate() {
        state_runtime.block_on(async {
            let state = AsyncSqliteStateStore::open(&database_path)
                .await
                .expect("fixture state");
            state
                .upsert_account(
                    &AccountRecord::new(
                        Provider::Claude,
                        account_id.clone(),
                        format!("claude-{index}"),
                        AccountStatus::Enabled,
                    )
                    .with_active_credential_generation(1),
                )
                .await
                .expect("fixture account");
        });
        let bundle = CredentialBundle::new_claude(
            SecretString::new(format!("claude-access-{index}")),
            SecretString::new(format!("claude-refresh-{index}")),
            10_000,
        )
        .expect("fixture bundle");
        secrets
            .write_secret(
                &provider_credential_bundle_key(Provider::Claude, account_id, 1).expect("key"),
                &bundle.to_secret_string().expect("bundle payload"),
            )
            .expect("fixture credential");
    }
    state_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .expect("async state");
        for (account_index, account_id) in account_ids.iter().enumerate() {
            for window_kind in [
                codex_router_core::route_profile::WindowKind::FiveHour,
                codex_router_core::route_profile::WindowKind::Weekly,
            ] {
                state
                    .record_window_observation(
                        &codex_router_state::window_observation::WindowObservation::new(
                            codex_router_state::window_observation::WindowObservationProps::new(
                                account_id.clone(),
                                window_kind,
                                if reserve_primary
                                    && account_index == 0
                                    && window_kind
                                        == codex_router_core::route_profile::WindowKind::FiveHour
                                {
                                    500
                                } else {
                                    10_000
                                },
                                1_000,
                            )
                            .with_reset_unix_seconds(2_000)
                            .with_fresh_until_unix_seconds(2_000),
                        )
                        .expect("fixture observation"),
                        || 1_100,
                    )
                    .await
                    .expect("fixture quota");
            }
        }
        state
            .upsert_session_account_affinity(&SessionAccountAffinity::with_pin_state(
                Provider::Claude,
                "claude-session",
                Some(account_ids[0].clone()),
                3,
                1_000,
            ))
            .await
            .expect("fixture pin");
    });
    let config = LoopbackRouterRuntimeConfig::new_tokenless(
        LoopbackBindAddress::new("127.0.0.1", 0).expect("router bind"),
        UpstreamEndpoint::new("http://127.0.0.1:1/v1").expect("unused OpenAI endpoint"),
        database_path.clone(),
        secret_root,
    )
    .with_claude_edge_local_token(
        local_router_token("claude-local", 1),
        scenario.quota_refresh_interval,
    )
    .with_quota_clock(1_100, 300)
    .with_debug_claude_upstream_endpoint(
        ClaudeUpstreamEndpoint::isolated_debug_override(format!("http://{upstream_address}"), true)
            .expect("isolated fixture endpoint"),
    );
    let runtime = LoopbackRouterRuntime::start(config, secrets).expect("fixture router");
    let router_address = runtime.local_addr();
    let router_thread = std::thread::spawn(move || runtime.serve_http_connections(1));
    let mut client = TcpStream::connect(router_address).expect("fixture client");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("client timeout");
    write!(client, "POST /anthropic/v1/messages?trace=fixture HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: {}\r\nAuthorization: Bearer claude-local\r\nX-Api-Key: client-key\r\nX-Claude-Code-Session-Id: claude-session\r\nAnthropic-Version: 2023-06-01\r\nAnthropic-Beta: client-capability\r\n\r\n", scenario.request_body.len()).expect("request headers");
    client
        .write_all(&scenario.request_body)
        .expect("request body");
    client
        .shutdown(std::net::Shutdown::Write)
        .expect("request complete");
    let mut response = String::new();
    client
        .read_to_string(&mut response)
        .expect("client response");
    assert_eq!(
        router_thread
            .join()
            .expect("router join")
            .expect("router result"),
        1
    );
    let requests = upstream_thread.join().expect("upstream join");
    ClaudeLoopbackReceipt {
        response,
        requests,
        database_path,
        account_ids,
    }
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
