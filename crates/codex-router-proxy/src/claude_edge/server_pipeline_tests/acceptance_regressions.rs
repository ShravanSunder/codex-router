//! Real-listener regression cases and their isolated fixture.

use super::{
    ClaudeLoopbackReceipt, ClaudeLoopbackScenario, FakeClaudeOAuthIssuer,
    FakeClaudeOAuthIssuerOutcome, assert_fake_claude_refresh_request, claude_credential_rejection,
    claude_fixture_response, client_response_body, mark_claude_primary_reserve,
    read_claude_credential_maintenance, read_claude_fixture_pin, read_until_response_marker,
    write_claude_fixture_chunk, write_claude_fixture_request,
    write_claude_fixture_request_with_declared_content_length,
};
use crate::server::{
    LoopbackBindAddress, LoopbackRouterRuntime, LoopbackRouterRuntimeConfig, read_http_request,
};
use crate::upstream::{ClaudeUpstreamEndpoint, UpstreamEndpoint};
use codex_router_auth::claude_oauth::ClaudeOAuthRefreshClient;
use codex_router_core::ids::{AccountId, TokenGeneration};
use codex_router_core::local_auth::LocalRouterTokenRecord;
use codex_router_core::redaction::SecretString;
use codex_router_secret_store::SecretStore;
use codex_router_secret_store::encrypted_credential_store::EncryptedCredentialStore;
use codex_router_secret_store::file_backend::FileSecretStore;
use codex_router_state::account::{AccountRecord, AccountStatus};
use codex_router_state::credential_maintenance::CredentialMaintenanceState;
use codex_router_state::sqlite::AsyncSqliteStateStore;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use tokio::net::TcpListener as TokioTcpListener;

fn local_router_token(token: &str, generation: u64) -> LocalRouterTokenRecord {
    LocalRouterTokenRecord::new(SecretString::new(token), TokenGeneration::new(generation))
}

fn run_claude_r9_scenario(
    case: &'static str,
    issuer_outcome: FakeClaudeOAuthIssuerOutcome,
) -> ClaudeLoopbackReceipt {
    let issuer = FakeClaudeOAuthIssuer::start(issuer_outcome);
    let refresh_client =
        ClaudeOAuthRefreshClient::with_test_issuer(issuer.endpoint.clone(), "test-claude-client");
    run_claude_loopback_scenario_with_refresh(
        ClaudeLoopbackScenario {
            case,
            request_body: b"{}".to_vec(),
            reserve_primary: false,
            reserve_primary_while_streaming: false,
            quota_refresh_interval: Duration::from_secs(180),
            responses: vec![
                claude_credential_rejection(),
                claude_fixture_response(
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    "{\"type\":\"message\"}",
                ),
            ],
        },
        Some((refresh_client, issuer)),
    )
}

fn assert_claude_attempt_authentication(
    receipt: &ClaudeLoopbackReceipt,
    second_account_token: &str,
) {
    assert!(receipt.response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(receipt.requests.len(), 2);
    assert_eq!(
        receipt.requests[0].header_value("authorization"),
        Some("Bearer claude-access-0")
    );
    assert!(
        receipt.requests[1]
            .header_value("authorization")
            .is_some_and(|value| value == second_account_token),
        "attempt two uses its required account credential"
    );
    assert_fake_claude_refresh_request(
        receipt
            .oauth_request
            .as_ref()
            .expect("one fake issuer request"),
        "claude-refresh-0",
    );
}

pub(super) fn run_claude_loopback_scenario(
    scenario: ClaudeLoopbackScenario,
) -> ClaudeLoopbackReceipt {
    run_claude_loopback_scenario_with_refresh(scenario, None)
}

fn run_claude_loopback_scenario_with_refresh(
    scenario: ClaudeLoopbackScenario,
    refresh: Option<(ClaudeOAuthRefreshClient, FakeClaudeOAuthIssuer)>,
) -> ClaudeLoopbackReceipt {
    run_claude_loopback_scenario_with_store(scenario, refresh, false, false)
}

fn run_claude_loopback_scenario_with_unavailable_credential_store(
    scenario: ClaudeLoopbackScenario,
) -> ClaudeLoopbackReceipt {
    run_claude_loopback_scenario_with_store(scenario, None, true, true)
}

fn run_claude_loopback_scenario_with_store(
    scenario: ClaudeLoopbackScenario,
    refresh: Option<(ClaudeOAuthRefreshClient, FakeClaudeOAuthIssuer)>,
    credential_store_unavailable: bool,
    incomplete_request_body: bool,
) -> ClaudeLoopbackReceipt {
    use codex_router_core::provider::Provider;
    use codex_router_secret_store::account_tokens::provider_credential_bundle_key;
    use codex_router_secret_store::credential_bundle::CredentialBundle;
    use codex_router_state::session_account_affinity::SessionAccountAffinity;

    let reserve_primary = scenario.reserve_primary;
    let reserve_primary_while_streaming = scenario.reserve_primary_while_streaming;
    let (stream_started_sender, stream_started_receiver) = std::sync::mpsc::sync_channel(1);
    let (resume_stream_sender, resume_stream_receiver) = std::sync::mpsc::sync_channel(1);
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
            for (response_index, response) in scenario.responses.into_iter().enumerate() {
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
                if reserve_primary_while_streaming && response_index == 0 {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")
                        .expect("stream headers");
                    write_claude_fixture_chunk(
                        &mut stream,
                        "event: message_start\ndata: {\"type\":\"message_start\"}\n\n",
                    );
                    stream_started_sender
                        .send(
                            requests[response_index]
                                .header_value("authorization")
                                .map(str::to_owned),
                        )
                        .expect("stream start receiver");
                    resume_stream_receiver
                        .recv_timeout(Duration::from_secs(3))
                        .expect("reserve transition before stream completion");
                    write_claude_fixture_chunk(
                        &mut stream,
                        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                    );
                    stream.write_all(b"0\r\n\r\n").expect("stream completion");
                } else if !response.is_empty() {
                    stream
                        .write_all(response.as_bytes())
                        .expect("upstream response");
                }
            }
            requests
        })
    });
    let temporary_directory = tempfile::Builder::new()
        .prefix(&format!("codex-router-claude-server-{}-", scenario.case))
        .tempdir()
        .expect("Claude fixture temp directory");
    let database_path = temporary_directory.path().join("router.sqlite");
    let secret_root = temporary_directory.path().join("secrets");
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
    let mut credential_patterns = vec![
        ("local router token".to_owned(), "claude-local".to_owned()),
        ("client API key".to_owned(), "client-key".to_owned()),
        (
            "rotated account access token".to_owned(),
            "claude-access-rotated".to_owned(),
        ),
        (
            "rotated account refresh token".to_owned(),
            "claude-refresh-rotated".to_owned(),
        ),
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
        let access_token = format!("claude-access-{index}");
        let refresh_token = format!("claude-refresh-{index}");
        credential_patterns.push((
            format!("account {index} access token"),
            access_token.clone(),
        ));
        credential_patterns.push((
            format!("account {index} refresh token"),
            refresh_token.clone(),
        ));
        let bundle = CredentialBundle::new_claude(
            SecretString::new(access_token),
            SecretString::new(refresh_token),
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
    let credential_store = if credential_store_unavailable {
        assert!(
            secrets
                .read_secret(
                    &provider_credential_bundle_key(Provider::Claude, &account_ids[0], 1)
                        .expect("active credential key")
                )
                .is_ok(),
            "the account has an active, readable credential before simulating key unavailability"
        );
        drop(secrets);
        EncryptedCredentialStore::key_unavailable(
            FileSecretStore::open(&secret_root).expect("unavailable credential store files"),
        )
    } else {
        secrets
    };
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
    let refresh_client = refresh
        .as_ref()
        .map(|(refresh_client, _issuer)| refresh_client.clone());
    let max_connections = if reserve_primary_while_streaming {
        2
    } else {
        1
    };
    let (router_address_sender, router_address_receiver) = std::sync::mpsc::sync_channel(1);
    let router_thread = std::thread::spawn(move || {
        let mut serve_result = None;
        let logs = crate::test_log_capture::capture_log_output(|| {
            let runtime = LoopbackRouterRuntime::start(config, credential_store.into())
                .expect("fixture router");
            let runtime = match refresh_client {
                Some(refresh_client) => runtime.with_test_claude_refresh_client(refresh_client),
                None => runtime,
            };
            router_address_sender
                .send(runtime.local_addr())
                .expect("router address receiver should be ready");
            serve_result = Some(runtime.serve_http_connections(max_connections));
        });
        (serve_result.expect("router result recorded"), logs)
    });
    let router_address = router_address_receiver
        .recv_timeout(Duration::from_secs(3))
        .expect("scoped listener runtime should publish its address before serving");
    let mut client = TcpStream::connect(router_address).expect("fixture client");
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("client timeout");
    if incomplete_request_body {
        write_claude_fixture_request_with_declared_content_length(
            &mut client,
            &scenario.request_body,
            scenario.request_body.len().saturating_add(1),
        );
    } else {
        write_claude_fixture_request(&mut client, &scenario.request_body);
    }
    let mut second_response = None;
    let mut response_bytes = if reserve_primary_while_streaming {
        assert_eq!(
            stream_started_receiver
                .recv_timeout(Duration::from_secs(3))
                .expect("upstream started first stream")
                .as_deref(),
            Some("Bearer claude-access-0")
        );
        let prefix = read_until_response_marker(&mut client, b"message_start");
        mark_claude_primary_reserve(&database_path, &account_ids[0]);
        resume_stream_sender
            .send(())
            .expect("resume streamed response");
        let mut response_bytes = prefix;
        client
            .read_to_end(&mut response_bytes)
            .expect("complete first stream response");

        let mut next_client = TcpStream::connect(router_address).expect("next fixture client");
        next_client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("next client timeout");
        write_claude_fixture_request(&mut next_client, &scenario.request_body);
        let mut next_response = String::new();
        next_client
            .read_to_string(&mut next_response)
            .expect("next client response");
        second_response = Some(next_response);
        response_bytes
    } else {
        let mut response_bytes = Vec::new();
        client
            .read_to_end(&mut response_bytes)
            .expect("client response");
        response_bytes
    };
    let response =
        String::from_utf8(std::mem::take(&mut response_bytes)).expect("fixture response utf-8");
    let (router_result, logs) = router_thread.join().expect("router join");
    assert_eq!(router_result.expect("router result"), max_connections);
    let requests = upstream_thread.join().expect("upstream join");
    let oauth_request = refresh.map(|(_client, issuer)| issuer.finish());
    let receipt = ClaudeLoopbackReceipt {
        response,
        second_response,
        logs,
        requests,
        oauth_request,
        database_path,
        _temporary_directory: temporary_directory,
        account_ids,
        credential_patterns,
    };
    if !credential_store_unavailable {
        assert!(
            receipt
                .logs
                .contains("codex_router.claude_attempt_selected"),
            "scoped subscriber captures an event from the spawned listener handler"
        );
    }
    super::assert_no_credential_patterns(
        &[
            ("client response", &receipt.response),
            (
                "second client response",
                receipt.second_response.as_deref().unwrap_or_default(),
            ),
            ("proxy logs", &receipt.logs),
        ],
        &receipt.credential_patterns,
    );
    receipt
}

#[test]
fn unavailable_credential_store_short_circuits_body_and_account_selection_for_active_credentials() {
    let receipt =
        run_claude_loopback_scenario_with_unavailable_credential_store(ClaudeLoopbackScenario {
            case: "claude-key-unavailable",
            request_body: b"{}".to_vec(),
            reserve_primary: false,
            reserve_primary_while_streaming: false,
            quota_refresh_interval: Duration::from_secs(180),
            responses: Vec::new(),
        });

    assert!(
        receipt
            .response
            .starts_with("HTTP/1.1 503 Service Unavailable")
    );
    assert!(receipt.response.contains("Keychain key is unreadable"));
    assert!(receipt.requests.is_empty());
    assert!(
        !receipt
            .logs
            .contains("codex_router.claude_attempt_selected")
    );
}

#[test]
fn claude_server_stream_completes_after_primary_enters_reserve_and_next_request_switches() {
    let body = b"{}".to_vec();
    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        case: "claude_server_reserve_during_stream",
        request_body: body.clone(),
        reserve_primary: false,
        reserve_primary_while_streaming: true,
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![
            String::new(),
            claude_fixture_response("200 OK", "Content-Type: application/json\r\n", "{}"),
        ],
    });

    assert!(receipt.response.starts_with("HTTP/1.1 200 OK"));
    assert!(receipt.response.contains("message_start"));
    assert!(receipt.response.contains("message_stop"));
    let next_response = receipt
        .second_response
        .as_deref()
        .expect("next request response");
    assert!(next_response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(receipt.requests.len(), 2);
    assert_eq!(receipt.requests[0].body(), body);
    assert_eq!(receipt.requests[1].body(), body);
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
fn claude_server_provider_unreachable_returns_exact_502_body_without_retry() {
    const EXPECTED_BODY: &str = r#"{"type":"error","error":{"type":"provider_unreachable","message":"Claude provider could not be reached."}}"#;

    let receipt = run_claude_loopback_scenario(ClaudeLoopbackScenario {
        case: "claude_server_provider_unreachable",
        request_body: b"{}".to_vec(),
        reserve_primary: false,
        reserve_primary_while_streaming: false,
        quota_refresh_interval: Duration::from_secs(180),
        responses: vec![String::new()],
    });

    assert_eq!(
        receipt.response.lines().next(),
        Some("HTTP/1.1 502 Bad Gateway")
    );
    assert_eq!(client_response_body(&receipt.response), EXPECTED_BODY);
    assert_eq!(receipt.requests.len(), 1);
    assert_eq!(
        receipt.requests[0].header_value("authorization"),
        Some("Bearer claude-access-0")
    );
}

#[test]
fn claude_server_successful_refresh_retries_same_account_with_rotated_credential() {
    let receipt = run_claude_r9_scenario(
        "claude_server_refresh_rotated",
        FakeClaudeOAuthIssuerOutcome::Rotated,
    );

    assert_claude_attempt_authentication(&receipt, "Bearer claude-access-rotated");
    let maintenance = read_claude_credential_maintenance(&receipt, &receipt.account_ids[0]);
    assert_eq!(maintenance.state, CredentialMaintenanceState::Healthy);
    assert_eq!(maintenance.credential_generation, 2);
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[0]));
    assert_eq!(pin.pin_version(), 3);
}

#[test]
fn claude_server_refused_refresh_marks_primary_needs_login_and_retries_secondary() {
    let receipt = run_claude_r9_scenario(
        "claude_server_refresh_refused",
        FakeClaudeOAuthIssuerOutcome::Refused,
    );

    assert_claude_attempt_authentication(&receipt, "Bearer claude-access-1");
    let maintenance = read_claude_credential_maintenance(&receipt, &receipt.account_ids[0]);
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
    assert_eq!(maintenance.credential_generation, 1);
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[1]));
    assert_eq!(pin.pin_version(), 5);
}

#[test]
fn claude_server_uncertain_refresh_marks_primary_needs_login_and_retries_secondary() {
    let receipt = run_claude_r9_scenario(
        "claude_server_refresh_uncertain",
        FakeClaudeOAuthIssuerOutcome::Uncertain,
    );

    assert_claude_attempt_authentication(&receipt, "Bearer claude-access-1");
    let maintenance = read_claude_credential_maintenance(&receipt, &receipt.account_ids[0]);
    assert_eq!(
        maintenance.state,
        CredentialMaintenanceState::ReauthRequired
    );
    assert_eq!(maintenance.credential_generation, 1);
    let pin = read_claude_fixture_pin(&receipt);
    assert_eq!(pin.account_id(), Some(&receipt.account_ids[1]));
    assert_eq!(pin.pin_version(), 5);
}
