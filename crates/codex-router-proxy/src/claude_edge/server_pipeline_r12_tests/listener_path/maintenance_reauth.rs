use super::account_id;
use super::assert_fake_upstream_received_no_request;
use super::fake_claude_upstream_probe;
use super::runtime_config;
use super::send_claude_request;
use super::serve_one_request;
use super::test_database_path;
use crate::http_sse::HttpProxyRequest;
use crate::server::LoopbackRouterRuntime;
use crate::server::read_http_request;
use crate::upstream::ClaudeUpstreamEndpoint;
use codex_router_auth::claude_oauth::ClaudeOAuthRefreshClient;
use codex_router_core::ids::AccountId;
use codex_router_core::provider::Provider;
use codex_router_core::redaction::SecretString;
use codex_router_core::route_profile::WindowKind;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
use codex_router_secret_store::credential_bundle::CredentialBundle;
use codex_router_state::account::AccountRecord;
use codex_router_state::account::AccountStatus;
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use codex_router_state::window_observation::WindowObservation;
use codex_router_state::window_observation::WindowObservationProps;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::thread;
use std::time::Duration;
use tokio::net::TcpListener as TokioTcpListener;

fn send_claude_request_for_session(address: std::net::SocketAddr, session_id: &str) -> String {
    let mut stream = TcpStream::connect(address)
        .unwrap_or_else(|error| panic!("fixture Claude client should connect: {error}"));
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap_or_else(|error| panic!("fixture client timeout should be set: {error}"));
    let request = format!(
        "POST /anthropic/v1/messages HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Length: 2\r\nAuthorization: Bearer claude-r12-local\r\nx-claude-code-session-id: {session_id}\r\n\r\n{{}}"
    );
    stream
        .write_all(request.as_bytes())
        .unwrap_or_else(|error| panic!("fixture Claude request should be sent: {error}"));
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .unwrap_or_else(|error| panic!("fixture Claude response should be readable: {error}"));
    response
}

fn fixture_json_response(status_line: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status_line}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn start_fake_claude_upstream(
    responses: Vec<String>,
) -> (
    ClaudeUpstreamEndpoint,
    thread::JoinHandle<Vec<HttpProxyRequest>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("fixture upstream should bind: {error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("fixture upstream should be nonblocking: {error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("fixture upstream address should be available: {error}"));
    let upstream_thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("fixture upstream runtime should build: {error}"));
        runtime.block_on(async move {
            let listener = TokioTcpListener::from_std(listener).unwrap_or_else(|error| {
                panic!("fixture upstream listener should become async: {error}")
            });
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                    .await
                    .unwrap_or_else(|_elapsed| panic!("fixture upstream should receive a request"))
                    .unwrap_or_else(|error| panic!("fixture upstream should accept: {error}"));
                let mut stream = stream.into_std().unwrap_or_else(|error| {
                    panic!("fixture upstream stream should become blocking: {error}")
                });
                stream.set_nonblocking(false).unwrap_or_else(|error| {
                    panic!("fixture upstream stream should be blocking: {error}")
                });
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap_or_else(|error| {
                        panic!("fixture upstream timeout should be set: {error}")
                    });
                requests.push(read_http_request(&mut stream).unwrap_or_else(|error| {
                    panic!("fixture upstream request should parse: {error}")
                }));
                stream
                    .write_all(response.as_bytes())
                    .unwrap_or_else(|error| {
                        panic!("fixture upstream response should be written: {error}")
                    });
            }
            requests
        })
    });
    let endpoint =
        ClaudeUpstreamEndpoint::isolated_debug_override(format!("http://{address}"), true)
            .unwrap_or_else(|error| {
                panic!("isolated fake Claude upstream should be valid: {error}")
            });
    (endpoint, upstream_thread)
}

fn start_fake_claude_refresh_refusal() -> (
    ClaudeOAuthRefreshClient,
    thread::JoinHandle<HttpProxyRequest>,
) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .unwrap_or_else(|error| panic!("fixture OAuth issuer should bind: {error}"));
    listener
        .set_nonblocking(true)
        .unwrap_or_else(|error| panic!("fixture OAuth issuer should be nonblocking: {error}"));
    let address = listener.local_addr().unwrap_or_else(|error| {
        panic!("fixture OAuth issuer address should be available: {error}")
    });
    let issuer_thread = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| panic!("fixture OAuth runtime should build: {error}"));
        runtime.block_on(async move {
            let listener = TokioTcpListener::from_std(listener).unwrap_or_else(|error| {
                panic!("fixture OAuth listener should become async: {error}")
            });
            let (stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap_or_else(|_elapsed| panic!("R9 refresh should reach the fake issuer"))
                .unwrap_or_else(|error| panic!("fixture OAuth issuer should accept: {error}"));
            let mut stream = stream.into_std().unwrap_or_else(|error| {
                panic!("fixture OAuth stream should become blocking: {error}")
            });
            stream
                .set_nonblocking(false)
                .unwrap_or_else(|error| panic!("fixture OAuth stream should be blocking: {error}"));
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap_or_else(|error| panic!("fixture OAuth timeout should be set: {error}"));
            let request = read_http_request(&mut stream)
                .unwrap_or_else(|error| panic!("fixture OAuth request should parse: {error}"));
            let refusal = fixture_json_response("400 Bad Request", r#"{"error":"invalid_grant"}"#);
            stream
                .write_all(refusal.as_bytes())
                .unwrap_or_else(|error| panic!("fixture OAuth refusal should be written: {error}"));
            request
        })
    });
    let client = ClaudeOAuthRefreshClient::with_test_issuer(
        format!("http://{address}/v1/oauth/token"),
        "b1-test-claude-client",
    );
    (client, issuer_thread)
}

async fn add_healthy_claude_account(
    state: &AsyncSqliteStateStore,
    id_suffix: &str,
    label: &str,
) -> AccountId {
    let account_id = account_id(id_suffix);
    state
        .upsert_account(
            &AccountRecord::new(
                Provider::Claude,
                account_id.clone(),
                label,
                AccountStatus::Enabled,
            )
            .with_active_credential_generation(1),
        )
        .await
        .unwrap_or_else(|error| panic!("fixture Claude account should be stored: {error}"));
    for window_kind in [WindowKind::FiveHour, WindowKind::Weekly] {
        state
            .record_window_observation(
                &WindowObservation::new(
                    WindowObservationProps::new(account_id.clone(), window_kind, 10_000, 1_000)
                        .with_reset_unix_seconds(5_000)
                        .with_fresh_until_unix_seconds(2_000),
                )
                .unwrap_or_else(|error| {
                    panic!("fixture quota observation should be valid: {error}")
                }),
                || 1_100,
            )
            .await
            .unwrap_or_else(|error| panic!("fixture Claude quota should be stored: {error}"));
    }
    account_id
}

fn store_claude_fixture_credential<S: SecretStore>(
    secret_store: &S,
    account_id: &AccountId,
    access_token: &str,
    refresh_token: &str,
) {
    let credential_bundle = CredentialBundle::new_claude(
        SecretString::new(access_token),
        SecretString::new(refresh_token),
        10_000,
    )
    .unwrap_or_else(|error| panic!("fixture Claude bundle should be valid: {error}"));
    secret_store
        .write_secret(
            &provider_credential_bundle_key(Provider::Claude, account_id, 1)
                .unwrap_or_else(|error| panic!("fixture Claude key should be valid: {error}")),
            &credential_bundle
                .to_secret_string()
                .unwrap_or_else(|error| panic!("fixture Claude bundle should serialize: {error}")),
        )
        .unwrap_or_else(|error| panic!("fixture Claude credential should be stored: {error}"));
}

#[test]
fn loopback_b1_credential_maintenance_refusal_excludes_account_on_next_request() {
    let database_path = test_database_path("b1-r9-next-request");
    let secret_root = database_path.with_extension("secrets");
    let credentials =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .unwrap_or_else(|error| panic!("fixture encrypted store should open: {error}"));
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    let (primary_account, secondary_account) = setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        let primary = add_healthy_claude_account(&state, "b1_primary", "B1 Claude Primary").await;
        let secondary =
            add_healthy_claude_account(&state, "b1_secondary", "B1 Claude Secondary").await;
        (primary, secondary)
    });
    store_claude_fixture_credential(
        &credentials,
        &primary_account,
        "claude-b1-primary-access",
        "claude-b1-primary-refresh",
    );
    store_claude_fixture_credential(
        &credentials,
        &secondary_account,
        "claude-b1-secondary-access",
        "claude-b1-secondary-refresh",
    );

    let rejected_response = fixture_json_response(
        "401 Unauthorized",
        r#"{"type":"error","error":{"type":"authentication_error","message":"OAuth token has expired. Please obtain a new token or refresh your existing token."}}"#,
    );
    let success_response = fixture_json_response("200 OK", r#"{"type":"message"}"#);
    let (upstream_endpoint, upstream_thread) = start_fake_claude_upstream(vec![
        rejected_response,
        success_response.clone(),
        success_response,
    ]);
    let (refresh_client, issuer_thread) = start_fake_claude_refresh_refusal();
    let runtime = LoopbackRouterRuntime::start(
        runtime_config(&database_path, &secret_root)
            .with_quota_clock(1_100, 300)
            .with_debug_claude_upstream_endpoint(upstream_endpoint),
        credentials,
    )
    .unwrap_or_else(|error| panic!("fixture Router should start: {error}"))
    .with_test_claude_refresh_client(refresh_client);
    let router_address = runtime.local_addr();
    let router_thread = thread::spawn(move || runtime.serve_http_connections(2));

    let first_response = send_claude_request(router_address);
    let next_response = send_claude_request(router_address);
    let served_connections = router_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture Router thread should join"))
        .unwrap_or_else(|error| panic!("fixture Router should serve both requests: {error}"));
    assert_eq!(served_connections, 2);
    assert!(
        first_response.starts_with("HTTP/1.1 200 OK"),
        "{first_response}"
    );
    assert!(
        next_response.starts_with("HTTP/1.1 200 OK"),
        "{next_response}"
    );

    let upstream_requests = upstream_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture upstream thread should join"));
    assert_eq!(upstream_requests.len(), 3);
    assert_eq!(
        upstream_requests[0].header_value("authorization"),
        Some("Bearer claude-b1-primary-access"),
        "the first request should begin on the primary account"
    );
    assert_eq!(
        upstream_requests[1].header_value("authorization"),
        Some("Bearer claude-b1-secondary-access"),
        "R9 should retry on the secondary account after refresh refusal"
    );
    assert_eq!(
        upstream_requests[2].header_value("authorization"),
        Some("Bearer claude-b1-secondary-access"),
        "the next request should skip the primary account after reauth is required"
    );

    let issuer_request = issuer_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture OAuth issuer thread should join"));
    let refresh_body = serde_json::from_slice::<serde_json::Value>(issuer_request.body())
        .unwrap_or_else(|error| panic!("fixture refresh request should be JSON: {error}"));
    assert_eq!(
        refresh_body["refresh_token"], "claude-b1-primary-refresh",
        "R9 refresh should use the primary account token"
    );
    let maintenance = setup_runtime.block_on(async {
        AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("maintenance state should open: {error}"))
            .load_credential_maintenance(&primary_account)
            .await
            .unwrap_or_else(|error| panic!("maintenance state should load: {error}"))
            .unwrap_or_else(|| panic!("R9 refusal should persist maintenance state"))
    });
    assert_eq!(maintenance.credential_generation, 1);
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
}

#[test]
fn loopback_b1_all_reauth_required_accounts_return_r12_three_and_name_accounts() {
    let database_path = test_database_path("b1-all-reauth-required");
    let secret_root = database_path.with_extension("secrets");
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        for (suffix, label) in [
            ("b1_login_one", "B1 Login One"),
            ("b1_login_two", "B1 Login Two"),
        ] {
            let account = add_healthy_claude_account(&state, suffix, label).await;
            assert!(
                state
                    .mark_generation_reauth_required(&account, 1)
                    .await
                    .unwrap_or_else(|error| panic!("fixture reauth state should persist: {error}")),
                "active generation should become reauth-required"
            );
        }
    });

    let (endpoint, probe_start, upstream_probe) = fake_claude_upstream_probe();
    let runtime = LoopbackRouterRuntime::start_for_test(
        runtime_config(&database_path, &secret_root).with_debug_claude_upstream_endpoint(endpoint),
    )
    .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));
    let response = serve_one_request(runtime);
    assert_fake_upstream_received_no_request(probe_start, upstream_probe);

    let (headers, body) = response
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("fixture Claude response should have a body separator"));
    assert!(
        headers.starts_with("HTTP/1.1 503 Service Unavailable"),
        "{response}"
    );
    let body: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|error| panic!("Claude selection response should be JSON: {error}"));
    let message = body["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("Claude selection message should be present"));
    assert!(message.contains("B1 Login One"), "{message}");
    assert!(message.contains("B1 Login Two"), "{message}");
    assert!(message.contains("account login"), "{message}");
}

#[test]
fn loopback_b1_reauth_required_pin_releases_and_selects_elsewhere() {
    const SESSION_ID: &str = "b1-reauth-pin-session";

    let database_path = test_database_path("b1-reauth-pin-release");
    let secret_root = database_path.with_extension("secrets");
    let credentials =
        codex_router_secret_store::test_support::open_encrypted_credential_store(&secret_root)
            .unwrap_or_else(|error| panic!("fixture encrypted store should open: {error}"));
    let setup_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| panic!("fixture setup runtime should build: {error}"));
    let (primary_account, secondary_account) = setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should open: {error}"));
        let primary = add_healthy_claude_account(&state, "b1_pin_primary", "B1 Pin Primary").await;
        let secondary =
            add_healthy_claude_account(&state, "b1_pin_secondary", "B1 Pin Secondary").await;
        (primary, secondary)
    });
    store_claude_fixture_credential(
        &credentials,
        &primary_account,
        "claude-b1-pin-primary-access",
        "claude-b1-pin-primary-refresh",
    );
    store_claude_fixture_credential(
        &credentials,
        &secondary_account,
        "claude-b1-pin-secondary-access",
        "claude-b1-pin-secondary-refresh",
    );
    let success_response = fixture_json_response("200 OK", r#"{"type":"message"}"#);
    let (upstream_endpoint, upstream_thread) =
        start_fake_claude_upstream(vec![success_response.clone(), success_response]);
    let first_runtime = LoopbackRouterRuntime::start(
        runtime_config(&database_path, &secret_root)
            .with_quota_clock(1_100, 300)
            .with_debug_claude_upstream_endpoint(upstream_endpoint.clone()),
        credentials.clone(),
    )
    .unwrap_or_else(|error| panic!("fixture Router should start: {error}"));
    let first_router_address = first_runtime.local_addr();
    let first_router_thread = thread::spawn(move || first_runtime.serve_http_connections(1));

    let first_response = send_claude_request_for_session(first_router_address, SESSION_ID);
    assert!(
        first_response.starts_with("HTTP/1.1 200 OK"),
        "{first_response}"
    );
    let first_served_connections = first_router_thread
        .join()
        .unwrap_or_else(|_error| panic!("first fixture Router thread should join"))
        .unwrap_or_else(|error| panic!("first fixture Router should drain: {error}"));
    assert_eq!(first_served_connections, 1);
    let first_pin = setup_runtime.block_on(async {
        AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture pin state should open: {error}"))
            .load_session_account_affinity(Provider::Claude, SESSION_ID)
            .await
            .unwrap_or_else(|error| panic!("fixture pin should load: {error}"))
            .unwrap_or_else(|| panic!("successful Claude response should publish a pin"))
    });
    assert_eq!(first_pin.account_id(), Some(&primary_account));

    setup_runtime.block_on(async {
        let state = AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture state should reopen: {error}"));
        assert!(
            state
                .mark_generation_reauth_required(&primary_account, 1)
                .await
                .unwrap_or_else(|error| panic!("fixture reauth state should persist: {error}")),
            "pinned active generation should become reauth-required"
        );
    });

    let second_runtime = LoopbackRouterRuntime::start(
        runtime_config(&database_path, &secret_root)
            .with_quota_clock(1_100, 300)
            .with_debug_claude_upstream_endpoint(upstream_endpoint),
        credentials,
    )
    .unwrap_or_else(|error| panic!("replacement fixture Router should start: {error}"));
    let second_router_address = second_runtime.local_addr();
    let second_router_thread = thread::spawn(move || second_runtime.serve_http_connections(1));
    let next_response = send_claude_request_for_session(second_router_address, SESSION_ID);
    let second_served_connections = second_router_thread
        .join()
        .unwrap_or_else(|_error| panic!("replacement fixture Router thread should join"))
        .unwrap_or_else(|error| panic!("replacement fixture Router should drain: {error}"));
    assert_eq!(second_served_connections, 1);
    assert!(
        next_response.starts_with("HTTP/1.1 200 OK"),
        "{next_response}"
    );

    let upstream_requests = upstream_thread
        .join()
        .unwrap_or_else(|_error| panic!("fixture upstream thread should join"));
    assert_eq!(upstream_requests.len(), 2);
    assert_eq!(
        upstream_requests[0].header_value("authorization"),
        Some("Bearer claude-b1-pin-primary-access"),
        "the first request should establish a pin to the primary account"
    );
    assert_eq!(
        upstream_requests[1].header_value("authorization"),
        Some("Bearer claude-b1-pin-secondary-access"),
        "a reauth-required pin should release and route to the secondary account"
    );
    let second_pin = setup_runtime.block_on(async {
        AsyncSqliteStateStore::open(&database_path)
            .await
            .unwrap_or_else(|error| panic!("fixture pin state should reopen: {error}"))
            .load_session_account_affinity(Provider::Claude, SESSION_ID)
            .await
            .unwrap_or_else(|error| panic!("fixture replacement pin should load: {error}"))
            .unwrap_or_else(|| panic!("successful replacement response should publish a pin"))
    });
    assert_eq!(second_pin.account_id(), Some(&secondary_account));
    assert_eq!(
        second_pin.pin_version(),
        first_pin.pin_version().saturating_add(2),
        "releasing the stale pin and publishing the replacement should each advance ownership"
    );
}
